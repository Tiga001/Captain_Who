import type { AgentEvent } from '@mycopilot/protocol'
import { applyAgentEventToChatMessage } from '../features/agentRun/agentEventReducer'
import type { ChatConversation, ChatMessage } from '../features/chat/chatTypes'

export function removeCoveredWorkflowMessages(messages: ChatMessage[]): ChatMessage[] {
  const deliveries = new Map<string, { inputId: string; instanceId: string }>()
  for (const message of messages) {
    if (message.role !== 'assistant') continue
    for (const item of message.agentRun?.timeline ?? []) {
      if (item.type === 'workflow_delivery') deliveries.set(item.deliveryId, item)
    }
  }
  // Only the independent UI copy of a formally applied delivery is covered. Startup mail,
  // claimed-but-unapplied mail and unrelated pending Composer input remain untouched.
  return messages.filter((message) => {
    const delivery = deliveries.get(message.id)
    return !(
      message.role === 'user' &&
      delivery &&
      message.workflowSource?.inputId === delivery.inputId &&
      message.workflowSource.instanceId === delivery.instanceId
    )
  })
}

/** A durable delivery may arrive after its active Run binding has been retired. */
export function applyWorkflowDeliveryToConversation(
  conversation: ChatConversation,
  event: Extract<AgentEvent, { type: 'workflow_delivery_applied' }>
): ChatConversation {
  if (conversation.id !== event.conversationId) return conversation
  const owner = conversation.messages.find(
    (message) =>
      message.id === event.assistantMessageId &&
      message.role === 'assistant' &&
      message.agentRun?.runId === event.runId
  )
  if (!owner) return conversation
  const updated = applyAgentEventToChatMessage(owner, event)
  return {
    ...conversation,
    messages: removeCoveredWorkflowMessages(
      conversation.messages.map((message) => (message === owner ? updated : message))
    )
  }
}
