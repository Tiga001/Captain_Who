import { isCompletedAgentRunStatus } from '../agentRun/agentEventReducerShared'
import type { ChatMessage } from './chatTypes'

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
