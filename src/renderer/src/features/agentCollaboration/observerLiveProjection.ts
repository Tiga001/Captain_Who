import type { AgentEvent, AgentObserverEventEnvelope } from '@mycopilot/protocol'
import { applyAgentEventToChatMessage } from '../agentRun/agentEventReducer'
import type { ChatConversation } from '../chat/chatTypes'

export interface ObserverScope {
  agentId: string
  rootAgentId: string
  rootConversationId: string
  conversationId: string
}

export function applyObserverLiveEnvelope(
  conversation: ChatConversation,
  scope: ObserverScope,
  envelope: AgentObserverEventEnvelope
): ChatConversation {
  if (
    envelope.agentId !== scope.agentId ||
    envelope.rootAgentId !== scope.rootAgentId ||
    envelope.rootConversationId !== scope.rootConversationId ||
    envelope.conversationId !== scope.conversationId ||
    conversation.id !== scope.conversationId ||
    envelope.event.runId !== envelope.runId
  ) {
    return conversation
  }
  const message = conversation.messages.find(
    (candidate) =>
      candidate.id === envelope.assistantMessageId && candidate.agentRun?.runId === envelope.runId
  )
  if (!message) return conversation
  const next = applyAgentEventToChatMessage(message, envelope.event)
  if (next === message) return conversation
  return {
    ...conversation,
    messages: conversation.messages.map((candidate) =>
      candidate.id === message.id ? next : candidate
    )
  }
}

/**
 * Command Sessions outlive their parent Run and already carry exact durable owner identities.
 * Keep consuming that legacy stream as a narrow compatibility overlay until the process exits;
 * every other child event must use the Host-authenticated observer envelope.
 */
export function applyObserverCommandEvent(
  conversation: ChatConversation,
  scope: ObserverScope,
  event: AgentEvent
): ChatConversation {
  if (
    event.type !== 'command_started' &&
    event.type !== 'command_output' &&
    event.type !== 'command_exited' &&
    event.type !== 'command_interrupted'
  ) {
    return conversation
  }
  if (event.conversationId !== scope.conversationId || conversation.id !== scope.conversationId) {
    return conversation
  }
  const message = conversation.messages.find(
    (candidate) =>
      candidate.id === event.assistantMessageId && candidate.agentRun?.runId === event.runId
  )
  if (!message) return conversation
  const next = applyAgentEventToChatMessage(message, event)
  if (next === message) return conversation
  return {
    ...conversation,
    messages: conversation.messages.map((candidate) =>
      candidate.id === message.id ? next : candidate
    )
  }
}
