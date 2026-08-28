import { describe, expect, it } from 'vitest'
import type { ChatAgentRunView, ChatMessage } from '../chatTypes'
import { isAssistantMessageGenerating } from '../assistantGeneration'

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
