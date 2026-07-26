import type { AgentTodoState } from '@mycopilot/protocol'
import type { ChatAgentRunView, ChatConversation } from './chatTypes'

export interface LatestAgentTodo {
  completedAt?: number
  runStatus?: ChatAgentRunView['status']
  todo: AgentTodoState
}

export function getLatestAgentTodo(conversation: ChatConversation): LatestAgentTodo | null {
  const message = conversation.messages[conversation.messages.length - 1]
  if (message?.role !== 'assistant') return null
  const run = message.agentRun
  if (!run?.todo?.items.length) return null

  return {
    completedAt: run.completedAt,
    runStatus: run.status,
    todo: run.todo
  }
}
