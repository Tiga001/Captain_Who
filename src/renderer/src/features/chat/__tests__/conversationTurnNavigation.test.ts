import { expect, it, vi } from 'vitest'
import {
  getConversationTurnNavigationItems,
  normalizeTurnNavigationPreview
} from '../conversationTurnNavigation'
import type { ChatMessage } from '../chatTypes'

vi.mock('../../../host/hostClient', () => ({
  hostClient: {}
}))

function userMessage(
  id: string,
  content: string,
  attachments?: ChatMessage['attachments']
): ChatMessage {
  return {
    id,
    role: 'user',
    content,
    attachments,
    createdAt: 1,
    status: 'sent'
  }
}

function assistantMessage(
  id: string,
  content: string,
  options: {
    messageStatus?: ChatMessage['status']
    runStatus?: NonNullable<ChatMessage['agentRun']>['status']
  } = {}
): ChatMessage {
  const runStatus = options.runStatus

  return {
    id,
    role: 'assistant',
    content,
    createdAt: 2,
    status: options.messageStatus ?? 'sent',
    agentRun: runStatus
      ? {
          runId: `run-${id}`,
          status: runStatus,
          toolDefinitions: [],
          toolCalls: [],
          toolResults: [],
          approvals: [],
          diffs: [],
          timeline: []
        }
      : undefined
  }
}

it('derives one navigation item per settled user turn and excludes the pending final turn', () => {
  const items = getConversationTurnNavigationItems([
    userMessage('user-1', 'First request'),
    assistantMessage('assistant-1', 'First answer', { runStatus: 'completed' }),
    userMessage('user-2', 'Second request'),
    assistantMessage('assistant-2', 'Partial answer', {
      messageStatus: 'pending',
      runStatus: 'running'
    })
  ])

  expect(items).toEqual([
    {
      favorited: false,
      id: 'user-1',
      userMessageId: 'user-1',
      userPreview: 'First request',
      assistantPreview: 'First answer'
    }
  ])
})

it('carries the user message favorite state into its navigation item', () => {
  const favoriteMessage = userMessage('user-1', 'Keep this turn')
  favoriteMessage.uiState = { favorited: true }

  const items = getConversationTurnNavigationItems([
    favoriteMessage,
    assistantMessage('assistant-1', 'Saved', { runStatus: 'completed' })
  ])

  expect(items[0]?.favorited).toBe(true)
})

it('does not create a key for a final user message that has no assistant reply', () => {
  const items = getConversationTurnNavigationItems([
    userMessage('user-1', 'Completed request'),
    assistantMessage('assistant-1', 'Completed answer', { runStatus: 'completed' }),
    userMessage('user-2', 'Still waiting for a reply')
  ])

  expect(items.map((item) => item.userMessageId)).toEqual(['user-1'])
})

it('does not create a key while the reply is waiting for approval', () => {
  const items = getConversationTurnNavigationItems([
    userMessage('user-1', 'Please edit the file'),
    assistantMessage('assistant-1', 'Waiting for approval', {
      runStatus: 'waiting_for_approval'
    })
  ])

  expect(items).toEqual([])
})

it('does not create a key when a newer assistant reply is still running', () => {
  const items = getConversationTurnNavigationItems([
    userMessage('user-1', 'Retry this request'),
    assistantMessage('assistant-old', 'Earlier completed reply', {
      runStatus: 'completed'
    }),
    assistantMessage('assistant-retry', 'New reply is still streaming', {
      messageStatus: 'pending',
      runStatus: 'running'
    })
  ])

  expect(items).toEqual([])
})

it('keeps keys for manually cancelled and abnormally failed terminal turns', () => {
  const items = getConversationTurnNavigationItems([
    userMessage('user-1', 'Stop this response'),
    assistantMessage('assistant-1', 'Partial response', {
      runStatus: 'cancelled'
    }),
    userMessage('user-2', 'This request will fail'),
    assistantMessage('assistant-2', 'Provider connection failed', {
      messageStatus: 'error',
      runStatus: 'failed'
    })
  ])

  expect(items.map((item) => item.userMessageId)).toEqual(['user-1', 'user-2'])
})

it('uses the final settled assistant message before the next user message', () => {
  const items = getConversationTurnNavigationItems([
    userMessage('user-1', 'Question'),
    assistantMessage('assistant-empty', '', { runStatus: 'completed' }),
    assistantMessage('assistant-final', '**Final** [answer](https://example.com)', {
      runStatus: 'completed'
    }),
    userMessage('user-2', 'Next question'),
    assistantMessage('assistant-2', 'Next answer', { runStatus: 'completed' })
  ])

  expect(items.map((item) => item.assistantPreview)).toEqual(['Final answer', 'Next answer'])
})

it('uses attachment names for an attachment-only user message', () => {
  const items = getConversationTurnNavigationItems([
    userMessage('user-1', 'Attachments: report.xlsx', [
      {
        id: 'attachment-1',
        kind: 'file',
        name: 'report.xlsx',
        sizeBytes: 10
      }
    ]),
    assistantMessage('assistant-1', 'Reviewed', { runStatus: 'completed' })
  ])

  expect(items[0]?.userPreview).toBe('report.xlsx')
})

it('normalizes common markdown into compact plain-text previews', () => {
  expect(
    normalizeTurnNavigationPreview(
      '# Result\n- Read [`file.ts`](https://example.com/file.ts)\n- `pnpm test` passed'
    )
  ).toBe('Result Read file.ts pnpm test passed')
})
