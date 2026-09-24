import { unwrapHostInvocation } from '@mycopilot/host-api'
import type {
  AgentCollaborationSettingsGetInput,
  AgentCollaborationSettingsUpdate,
  AgentCollaborationSettings,
  AgentEvent,
  AgentConversationLocator,
  AgentConversationLocatorRequest,
  AgentDetail,
  AgentDetailRequest,
  AgentObserverConversationRequest,
  AgentObserverConversation,
  AgentObserverEventEnvelope,
  AgentTemplate,
  AgentTemplateCreateRequest,
  AgentTemplateDeleteRequest,
  AgentTemplateList,
  AgentTemplateListRequest,
  AgentTemplateProjectAssignmentRequest,
  AgentTemplateSetEnabledRequest,
  AgentTemplateUpdateRequest,
  AgentTreeRequest,
  AgentTreeSnapshot,
  CollaborationApprovalDecisionRequest,
  CollaborationApprovalDecisionResult,
  CollaborationApprovalList,
  CollaborationApprovalListRequest,
  CollaborationEventEnvelope,
  CollaborationEventsPage,
  CollaborationEventsRequest
} from '@mycopilot/protocol'
import { hostClient } from '../../host/hostClient'
import { onAgentEvent } from '../agent/agentClient'

export interface CollaborationDataSource {
  getTree(input: AgentTreeRequest): Promise<AgentTreeSnapshot | null>
  listEvents(input: CollaborationEventsRequest): Promise<CollaborationEventsPage>
  subscribe(handler: (event: CollaborationEventEnvelope) => void): () => void
  subscribeResync(handler: () => void): () => void
  subscribeAgentEvents?(handler: (event: AgentEvent) => void): () => void
}

export const hostCollaborationDataSource: CollaborationDataSource = {
  async getTree(input) {
    return unwrapHostInvocation(await hostClient.agent.getCollaborationTree(input)).tree
  },
  async listEvents(input) {
    return unwrapHostInvocation(await hostClient.agent.listCollaborationEvents(input))
  },
  subscribe(handler) {
    return hostClient.agent.onCollaborationEvent(handler)
  },
  subscribeResync(handler) {
    return hostClient.agent.onCollaborationResync(() => handler())
  },
  subscribeAgentEvents(handler) {
    return onAgentEvent(handler)
  }
}

export async function getCollaborationAgent(input: AgentDetailRequest): Promise<AgentDetail> {
  return unwrapHostInvocation(await hostClient.agent.getCollaborationAgent(input))
}

export async function locateCollaborationConversation(
  input: AgentConversationLocatorRequest
): Promise<AgentConversationLocator> {
  return unwrapHostInvocation(await hostClient.agent.locateCollaborationConversation(input))
}

export async function loadCollaborationObserverConversation(
  input: AgentObserverConversationRequest
): Promise<AgentObserverConversation | null> {
  return unwrapHostInvocation(await hostClient.agent.loadCollaborationObserverConversation(input))
}

export function onCollaborationObserverEvent(
  handler: (event: AgentObserverEventEnvelope) => void
): () => void {
  return hostClient.agent.onCollaborationObserverEvent(handler)
}

export async function listAgentTemplates(
  input: AgentTemplateListRequest
): Promise<AgentTemplateList> {
  return unwrapHostInvocation(await hostClient.agent.listAgentTemplates(input))
}

export async function createAgentTemplate(
  input: AgentTemplateCreateRequest
): Promise<AgentTemplate> {
  return unwrapHostInvocation(await hostClient.agent.createAgentTemplate(input))
}

export async function updateAgentTemplate(
  input: AgentTemplateUpdateRequest
): Promise<AgentTemplate> {
  return unwrapHostInvocation(await hostClient.agent.updateAgentTemplate(input))
}

export async function setAgentTemplateEnabled(
  input: AgentTemplateSetEnabledRequest
): Promise<AgentTemplate> {
  return unwrapHostInvocation(await hostClient.agent.setAgentTemplateEnabled(input))
}

export async function setAgentTemplateProjectAssignment(
  input: AgentTemplateProjectAssignmentRequest
): Promise<AgentTemplate> {
  return unwrapHostInvocation(await hostClient.agent.setAgentTemplateProjectAssignment(input))
}

export async function deleteAgentTemplate(
  input: AgentTemplateDeleteRequest
): Promise<AgentTemplate> {
  return unwrapHostInvocation(await hostClient.agent.deleteAgentTemplate(input))
}

export async function listCollaborationApprovals(
  input: CollaborationApprovalListRequest
): Promise<CollaborationApprovalList> {
  return unwrapHostInvocation(await hostClient.agent.listCollaborationApprovals(input))
}

export async function decideCollaborationApproval(
  input: CollaborationApprovalDecisionRequest
): Promise<CollaborationApprovalDecisionResult> {
  return unwrapHostInvocation(await hostClient.agent.decideCollaborationApproval(input))
}

export async function getCollaborationSettings(
  input: AgentCollaborationSettingsGetInput = {}
): Promise<AgentCollaborationSettings> {
  return unwrapHostInvocation(await hostClient.agent.getCollaborationSettings(input))
}
export async function updateCollaborationSettings(
  input: AgentCollaborationSettingsUpdate
): Promise<AgentCollaborationSettings> {
  return unwrapHostInvocation(await hostClient.agent.updateCollaborationSettings(input))
}
export function onCollaborationSettingsChanged(
  handler: (settings: AgentCollaborationSettings) => void
): () => void {
  return hostClient.agent.onCollaborationSettingsChanged(handler)
}
