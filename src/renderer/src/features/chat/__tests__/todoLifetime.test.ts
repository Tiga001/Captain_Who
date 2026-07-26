import { describe, expect, it } from 'vitest'
import type { ChatAgentRunView, ChatConversation, ChatMessage } from '../chatTypes'
import { getLatestAgentTodo } from '../todoLifetime'

function runWithTodo(runId: string): ChatAgentRunView {
  return {
    runId,
    status: 'running',
    toolDefinitions: [],
    todo: {
      revision: 1,
      items: [
        {
          id: 'todo-1',
          title: 'Current run only',
          status: 'in_progress',
          createdAt: 1,
          updatedAt: 1
        }
      ],
      updatedAt: 1
    },
    toolCalls: [],
    toolResults: [],
    approvals: [],
    diffs: [],
    timeline: []
  }
}

function message(id: string, role: ChatMessage['role'], agentRun?: ChatAgentRunView): ChatMessage {
  return {
    id,
    role,
    content: id,
    createdAt: 1,
    status: role === 'assistant' ? 'pending' : 'sent',
    agentRun
  }
}

function conversation(messages: ChatMessage[]): ChatConversation {
  return {
    id: 'conversation-1',
    projectId: null,
    modelId: 'model-1',
    title: 'Todo lifetime',
    messages,
    messagesLoaded: true,
    createdAt: 1,
    updatedAt: 1
  }
}

describe('run-scoped todo lifetime', () => {
  it('shows the todo only when it belongs to the latest assistant run', () => {
    const current = getLatestAgentTodo(
      conversation([
        message('user-1', 'user'),
        message('assistant-1', 'assistant', runWithTodo('run-1'))
      ])
    )

    expect(current?.todo.items[0]?.title).toBe('Current run only')
  })

  it('does not carry an older todo past a new user message', () => {
    const current = getLatestAgentTodo(
      conversation([
        message('user-1', 'user'),
        message('assistant-1', 'assistant', runWithTodo('run-1')),
        message('user-2', 'user')
      ])
    )

    expect(current).toBeNull()
  })

  it('does not fall back to an older todo while the new run has no plan', () => {
    const current = getLatestAgentTodo(
      conversation([
        message('user-1', 'user'),
        message('assistant-1', 'assistant', runWithTodo('run-1')),
        message('user-2', 'user'),
        message('assistant-2', 'assistant', {
          ...runWithTodo('run-2'),
          todo: undefined
        })
      ])
    )

    expect(current).toBeNull()
  })
})
