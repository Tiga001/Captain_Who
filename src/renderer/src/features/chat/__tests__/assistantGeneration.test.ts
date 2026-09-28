import { describe, expect, it } from 'vitest'
import type { ChatAgentRunView, ChatConversation, ChatMessage } from '../chatTypes'
import {
  getContinuableAssistantMessage,
  isAssistantMessageGenerating
} from '../assistantGeneration'

function assistant(
  status: ChatMessage['status'],
  runStatus?: ChatAgentRunView['status']
): ChatMessage {
  return {
    id: 'assistant-1',
    role: 'assistant',
    content: '',
    createdAt: 1,
    status,
    ...(runStatus === undefined
      ? {}
      : {
          agentRun: {
            runId: 'run-1',
            status: runStatus,
            startedAt: 1,
            toolDefinitions: [],
            toolCalls: [],
            toolResults: [],
            approvals: [],
            fileChangeProposals: [],
            fileChanges: [],
            fileChangePreviews: [],
            messageStreamCheckpoints: {},
            webSearchActivities: [],
            readActivities: [],
            mcpInvocations: [],
            timeline: []
          }
        })
  }
}

describe('isAssistantMessageGenerating', () => {
  it('treats a pending running assistant as generating', () => {
    expect(isAssistantMessageGenerating(assistant('pending', 'running'))).toBe(true)
  })

  it('does not treat a cancelled pending assistant as generating', () => {
    expect(isAssistantMessageGenerating(assistant('pending', 'cancelled'))).toBe(false)
  })

  it('does not treat a sent cancelled assistant as generating', () => {
    expect(isAssistantMessageGenerating(assistant('sent', 'cancelled'))).toBe(false)
  })
})

describe('getContinuableAssistantMessage', () => {
  const conversation = (messages: ChatMessage[]) =>
    ({
      id: 'conversation',
      messages,
      messagesLoaded: true
    }) as ChatConversation
  it('requires a durable explicit user stop on the latest message', () => {
    const stopped = assistant('sent', 'cancelled')
    const userStopped = { ...stopped, agentRun: { ...stopped.agentRun!, userInterrupted: true } }
    expect(getContinuableAssistantMessage(conversation([stopped]))).toBeUndefined()
    expect(getContinuableAssistantMessage(conversation([userStopped]))).toBe(userStopped)
    expect(
      getContinuableAssistantMessage(conversation([userStopped, assistant('sent', 'completed')]))
    ).toBeUndefined()
    expect(
      getContinuableAssistantMessage(conversation([userStopped, assistant('pending', 'running')]))
    ).toBeUndefined()
    expect(
      getContinuableAssistantMessage({ ...conversation([userStopped]), archivedAt: 1 })
    ).toBeUndefined()
    expect(
      getContinuableAssistantMessage({ ...conversation([userStopped]), messagesLoaded: false })
    ).toBeUndefined()
  })
})
