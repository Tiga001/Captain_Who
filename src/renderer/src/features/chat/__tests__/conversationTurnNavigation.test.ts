import { expect, it, vi } from 'vitest'
import {
  createConversationTurnNavigationSelector,
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
          fileChangeProposals: [],
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

it.each(['waiting_for_approval', 'waiting_for_user_input'] as const)(
  'does not create a key while the reply is %s',
  (runStatus) => {
    const items = getConversationTurnNavigationItems([
      userMessage('user-1', 'Please edit the file'),
      assistantMessage('assistant-1', 'Waiting for approval', {
        runStatus
      })
    ])

    expect(items).toEqual([])
  }
)

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

it('reuses all settled previews across 100 streaming updates to a 500-turn conversation', () => {
  const normalize = vi.fn(normalizeTurnNavigationPreview)
  const selectItems = createConversationTurnNavigationSelector(normalize)
  const completedMessages = Array.from({ length: 500 }, (_, index) => [
    userMessage(`user-${index}`, `**Question ${index}**`),
    assistantMessage(`assistant-${index}`, `**Answer ${index}**`, { runStatus: 'completed' })
  ]).flat()
  const pendingUser = userMessage('user-pending', 'Next question')
  const initialItems = selectItems(completedMessages)
  expect(normalize).toHaveBeenCalledTimes(1000)
  normalize.mockClear()

  for (let update = 0; update < 100; update += 1) {
    expect(
      selectItems([
        ...completedMessages,
        pendingUser,
        assistantMessage('assistant-pending', `Growing reply ${update}`, {
          messageStatus: 'pending',
          runStatus: 'running'
        })
      ])
    ).toEqual(initialItems)
  }
  expect(normalize).not.toHaveBeenCalled()
})

it('invalidates edited content and preserves live favorites and attachment fallback names', () => {
  const normalize = vi.fn(normalizeTurnNavigationPreview)
  const selectItems = createConversationTurnNavigationSelector(normalize)
  const user = userMessage('user', '', [
    { id: 'file', kind: 'file', name: 'first.txt', sizeBytes: 10 }
  ])
  const assistant = assistantMessage('assistant', 'Before', { runStatus: 'completed' })
  expect(selectItems([user, assistant])[0]?.userPreview).toBe('first.txt')
  normalize.mockClear()

  user.uiState = { favorited: true }
  user.attachments![0].name = 'renamed.txt'
  expect(selectItems([user, assistant])[0]).toMatchObject({
    favorited: true,
    userPreview: 'renamed.txt'
  })
  expect(normalize).not.toHaveBeenCalled()

  assistant.content = '**Changed in place**'
  expect(selectItems([user, assistant])[0]?.assistantPreview).toBe('Changed in place')
  expect(normalize).toHaveBeenCalledTimes(1)
  const editedUser = { ...user, content: '**Changed same ID**' }
  expect(selectItems([editedUser, assistant])[0]?.userPreview).toBe('Changed same ID')
  expect(normalize).toHaveBeenCalledTimes(2)
})

it('reflects lifecycle transitions, retries, deletion, reload, and independent conversations', () => {
  const selectItems = createConversationTurnNavigationSelector()
  const user = userMessage('user', 'Question')
  const assistant = assistantMessage('assistant', 'Partial', {
    messageStatus: 'pending',
    runStatus: 'running'
  })
  expect(selectItems([user, assistant])).toEqual([])
  assistant.status = 'sent'
  assistant.agentRun!.status = 'completed'
  expect(selectItems([user, assistant])[0]?.assistantPreview).toBe('Partial')
  assistant.agentRun!.status = 'cancelled'
  expect(selectItems([user, assistant])[0]?.assistantPreview).toBe('')
  assistant.agentRun!.status = 'failed'
  assistant.content = 'Failure detail'
  expect(selectItems([user, assistant])[0]?.assistantPreview).toBe('Failure detail')

  const retry = assistantMessage('retry', 'Still running', { runStatus: 'running' })
  expect(selectItems([user, assistant, retry])).toEqual([])
  expect(selectItems([user])).toEqual([])
  expect(selectItems([])).toEqual([])
  const reloaded = structuredClone([user, assistant])
  reloaded[1].content = 'Reloaded answer'
  expect(selectItems(reloaded)[0]?.assistantPreview).toBe('Reloaded answer')
  const otherConversation = createConversationTurnNavigationSelector()
  expect(
    otherConversation([
      userMessage('user', 'Other question'),
      assistantMessage('assistant', 'Other answer')
    ])[0]
  ).toMatchObject({ userPreview: 'Other question', assistantPreview: 'Other answer' })
})

it.runIf(process.env.RUN_PERFORMANCE_BENCHMARKS === '1')(
  'reports cold and hot navigation timings for the same synthetic history',
  () => {
    const content =
      '# Result\n- Read [`file.ts`](https://example.com/file.ts)\n- **Verified** `pnpm test`\n'.repeat(
        56
      )
    const median = (values: number[]) => values.sort((a, b) => a - b)[Math.floor(values.length / 2)]
    const results = [100, 500, 1000].map((turns) => {
      const messages = Array.from({ length: turns }, (_, index) => [
        userMessage(`user-${index}`, content),
        assistantMessage(`assistant-${index}`, content, { runStatus: 'completed' })
      ]).flat()
      const cached = createConversationTurnNavigationSelector()
      const cold: number[] = []
      const hot: number[] = []
      const uncached: number[] = []
      cached(messages)
      for (let sample = 0; sample < 15; sample += 1) {
        let start = performance.now()
        getConversationTurnNavigationItems(messages)
        uncached.push(performance.now() - start)
        start = performance.now()
        createConversationTurnNavigationSelector()(messages)
        cold.push(performance.now() - start)
        start = performance.now()
        cached([...messages])
        hot.push(performance.now() - start)
      }
      expect(cached(messages)).toEqual(getConversationTurnNavigationItems(messages))
      return {
        turns,
        charsPerMessage: content.length,
        uncachedMs: median(uncached),
        cachedColdMs: median(cold),
        cachedHotMs: median(hot)
      }
    })
    console.info('navigation preview timings', results)
  }
)
