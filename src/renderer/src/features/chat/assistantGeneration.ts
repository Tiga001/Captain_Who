import { isCompletedAgentRunStatus } from '../agentRun/agentEventReducerShared'
import type { ChatConversation, ChatMessage } from './chatTypes'

export function isAssistantReplyComplete(message: ChatMessage | undefined): boolean {
  return message?.status === 'sent' && isAssistantReplySettled(message)
}

/** Latest forks can preserve a terminal failure, but cannot snapshot an unfinished reply. */
export function isAssistantReplySettled(message: ChatMessage | undefined): boolean {
  if (!message || message.role !== 'assistant' || message.status === 'pending') return false
  const status = message.agentRun?.status
  return !status || status === 'idle' || isCompletedAgentRunStatus(status)
}

export function isAssistantMessageGenerating(message: ChatMessage): boolean {
  if (
    message.role !== 'assistant' ||
    (message.status !== 'pending' && message.agentRun?.status !== 'waiting_for_user_input')
  )
    return false
  return !isCompletedAgentRunStatus(message.agentRun?.status)
}

/** Only the latest durable user-interrupted turn can be explicitly continued. */
export function getContinuableAssistantMessage(
  conversation: ChatConversation | null | undefined
): ChatMessage | undefined {
  if (
    !conversation ||
    conversation.messagesLoaded === false ||
    conversation.archivedAt ||
    conversation.pendingArchivedAt !== undefined
  )
    return undefined
  const latest = conversation.messages.at(-1)
  return latest?.role === 'assistant' &&
    latest.status === 'sent' &&
    latest.agentRun?.status === 'cancelled' &&
    latest.agentRun.userInterrupted &&
    latest.agentRun.runId
    ? latest
    : undefined
}
