import { isCompletedAgentRunStatus } from '../agentRun/agentEventReducerShared'
import type { ChatMessage } from './chatTypes'

export function isAssistantMessageGenerating(message: ChatMessage): boolean {
  if (message.role !== 'assistant' || message.status !== 'pending') return false
  return !isCompletedAgentRunStatus(message.agentRun?.status)
}
