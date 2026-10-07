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

function workflowMessage(id: string, body?: string): ChatMessage {
  return {
    ...userMessage(id, 'Protocol header: organization inbox envelope'),
    workflowSource: {
      inputId: `input-${id}`,
      instanceId: 'organization',
      workflowName: 'Research team',
      sources: [
        {
          nodeId: 'researcher',
          nodeName: 'Researcher',
          conversationId: 'research-chat',
          conversationTitle: 'Evidence review',
          ...(body === undefined ? {} : { content: body })
        }
      ]
    }
  }
}

function withOrigin(
  message: ChatMessage,
  kind: NonNullable<ChatMessage['inputOrigin']>['kind'],
  senderAgentId: string | null = null
): ChatMessage {
  return {
    ...message,
    inputOrigin: {
      kind,
      senderAgentId,
      sourceAgentMessageId: null,
      snapshotSourceConversationId: null,
      snapshotSourceMessageId: null
    }
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
      source: 'human',
      userPreview: 'First request',
      assistantPreview: 'First answer'
    }
  ])
})

it('classifies authoritative input sources and preserves legacy human messages without reading body labels', () => {
  const inputs = [
    userMessage('legacy', 'From organization: this is text written by a human'),
    withOrigin(userMessage('human', 'Human request'), 'human'),
    withOrigin(userMessage('agent', 'Agent request'), 'agent', 'sender-agent'),
    withOrigin(userMessage('snapshot', 'Inherited context'), 'historical_snapshot', 'parent-agent'),
    withOrigin(userMessage('snapshot-without-sender', 'Older context'), 'historical_snapshot'),
    withOrigin(workflowMessage('workflow', 'Organization body'), 'agent', 'shadowed-agent')
  ]
  const items = getConversationTurnNavigationItems(
    inputs.flatMap((input) => [input, assistantMessage(`answer-${input.id}`, 'Done')])
  )
  expect(items).toMatchObject([
    { id: 'legacy', source: 'human' },
    { id: 'human', source: 'human' },
    { id: 'agent', source: 'agent', senderAgentId: 'sender-agent' },
    { id: 'snapshot', source: 'context', senderAgentId: 'parent-agent' },
    { id: 'snapshot-without-sender', source: 'context' },
    { id: 'workflow', source: 'workflow', sourceLabel: 'Research team · Researcher' }
  ])
  expect(items[4].senderAgentId).toBeUndefined()
  expect(items[5].senderAgentId).toBeUndefined()
})

it('previews original organization bodies and falls back to source names for legacy receipts', () => {
  const structured = workflowMessage('structured', '**Evidence** is ready.')
  structured.workflowSource!.sources.push({
    nodeId: 'reviewer',
    nodeName: 'Reviewer',
    conversationId: 'review-chat',
    conversationTitle: 'Review',
    content: 'Check `report.md`.'
  })
  const legacy = workflowMessage('legacy-mail')
  const partial = workflowMessage('partial', 'Only one known body')
  partial.workflowSource!.sources.push({
    nodeId: 'researcher-2',
    nodeName: 'Researcher',
    conversationId: 'other-research-chat',
    conversationTitle: 'Other evidence'
  })
  const items = getConversationTurnNavigationItems(
    [structured, legacy, partial].flatMap((input) => [
      input,
      assistantMessage(`answer-${input.id}`, 'Received')
    ])
  )
  expect(items.map((item) => item.userPreview)).toEqual([
    'Evidence is ready. Check report.md.',
    'Research team · Researcher',
    'Research team · Researcher'
  ])
  expect(items[0].sourceLabel).toBe('Research team · Researcher · Reviewer')
  expect(items.every((item) => !item.userPreview.includes('Protocol header'))).toBe(true)
})

it('keeps every consecutive human and mail input anchored to its own message with the shared final reply', () => {
  const first = userMessage('human-first', 'Initial request')
  first.uiState = { favorited: true }
  const mail = workflowMessage('mail', 'New findings')
  const last = userMessage('human-followup', 'Consider these findings')
  const items = getConversationTurnNavigationItems([
    first,
    mail,
    last,
    assistantMessage('earlier-answer', 'Earlier answer'),
    assistantMessage('final-answer', '**Final answer**'),
    userMessage('pending-human', 'Next request'),
    workflowMessage('pending-mail', 'New pending findings'),
    assistantMessage('pending-answer', 'Partial', { runStatus: 'running' })
  ])
  expect(items.map((item) => [item.id, item.userMessageId, item.source])).toEqual([
    ['human-first', 'human-first', 'human'],
    ['mail', 'mail', 'workflow'],
    ['human-followup', 'human-followup', 'human']
  ])
  expect(items.map((item) => item.assistantPreview)).toEqual([
    'Final answer',
    'Final answer',
    'Final answer'
  ])
  expect(items.map((item) => item.favorited)).toEqual([true, false, false])
})

it('does not borrow a later reply across an existing unfinished reply or expose a group during retry', () => {
  const human = userMessage('human', 'Original request')
  const mail = workflowMessage('mail', 'Update')
  expect(
    getConversationTurnNavigationItems([
      human,
      mail,
      assistantMessage('old-answer', 'Old answer'),
      assistantMessage('retry', 'Retrying', { runStatus: 'running' })
    ])
  ).toEqual([])
  expect(getConversationTurnNavigationItems([human, mail])).toEqual([])
  const items = getConversationTurnNavigationItems([
    human,
    mail,
    assistantMessage('unfinished', 'Still pending', { runStatus: 'running' }),
    userMessage('later-human', 'Independent later request'),
    assistantMessage('later-answer', 'Later answer')
  ])
  expect(items.map((item) => item.id)).toEqual(['later-human'])
})

it('shares normalization work across a large consecutive input group and reuses it during later streaming', () => {
  const normalize = vi.fn(normalizeTurnNavigationPreview)
  const selectItems = createConversationTurnNavigationSelector(normalize)
  const inputs = Array.from({ length: 500 }, (_, index) =>
    index % 2 === 0
      ? userMessage(`human-${index}`, `**Request ${index}**`)
      : workflowMessage(`mail-${index}`, `**Finding ${index}**`)
  )
  const settled = [...inputs, assistantMessage('answer', '**Shared answer**')]
  const initialItems = selectItems(settled)
  expect(initialItems).toHaveLength(inputs.length)
  expect(normalize).toHaveBeenCalledTimes(inputs.length + 1)
  normalize.mockClear()
  const nextInput = userMessage('next-human', 'Next request')
  for (let update = 0; update < 20; update += 1) {
    expect(
      selectItems([
        ...settled,
        nextInput,
        assistantMessage('next-answer', `Streaming ${update}`, { runStatus: 'running' })
      ])
    ).toEqual(initialItems)
  }
  expect(normalize).not.toHaveBeenCalled()
})

it('refreshes structured bodies and source metadata independently from cached preview text', () => {
  const normalize = vi.fn(normalizeTurnNavigationPreview)
  const selectItems = createConversationTurnNavigationSelector(normalize)
  const input = workflowMessage('mail', '**Before**')
  const answer = assistantMessage('answer', 'Received')
  selectItems([input, answer])
  normalize.mockClear()
  input.workflowSource!.workflowName = 'Renamed team'
  input.workflowSource!.sources[0].nodeName = 'Renamed researcher'
  expect(selectItems([input, answer])[0]).toMatchObject({
    source: 'workflow',
    sourceLabel: 'Renamed team · Renamed researcher',
    userPreview: 'Before'
  })
  expect(normalize).not.toHaveBeenCalled()
  input.workflowSource!.sources[0].content = '**After**'
  expect(selectItems([input, answer])[0]?.userPreview).toBe('After')
  expect(normalize).toHaveBeenCalledTimes(1)
  input.workflowSource = undefined
  input.inputOrigin = withOrigin(input, 'agent', 'new-sender').inputOrigin
  expect(selectItems([input, answer])[0]).toMatchObject({
    source: 'agent',
    senderAgentId: 'new-sender'
  })
  expect(selectItems(structuredClone([input, answer]))[0]).toMatchObject({
    source: 'agent',
    senderAgentId: 'new-sender'
  })
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
