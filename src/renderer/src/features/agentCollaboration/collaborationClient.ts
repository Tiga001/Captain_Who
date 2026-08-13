import { unwrapHostInvocation } from '@mycopilot/host-api'
import type {
  AgentConversationLocator,
  AgentConversationLocatorRequest,
  AgentDetail,
  AgentDetailRequest,
  AgentObserverConversationRequest,
  AgentObserverConversation,
  AgentTemplate,
  AgentTemplateCreateRequest,
  AgentTemplateDeleteRequest,
  AgentTemplateList,
  AgentTemplateListRequest,
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

export interface CollaborationDataSource {
  getTree(input: AgentTreeRequest): Promise<AgentTreeSnapshot | null>
  listEvents(input: CollaborationEventsRequest): Promise<CollaborationEventsPage>
  subscribe(handler: (event: CollaborationEventEnvelope) => void): () => void
  subscribeResync(handler: () => void): () => void
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
