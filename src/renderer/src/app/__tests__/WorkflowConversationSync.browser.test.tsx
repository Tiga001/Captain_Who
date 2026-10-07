import { useCallback, useRef, useState, type SetStateAction } from 'react'
import { act } from 'react'
import { beforeEach, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { WorkflowRuntimeSnapshot } from '@mycopilot/protocol'
import type { ChatConversation } from '../../features/chat/chatTypes'
import { useWorkflowConversationSync } from '../useWorkflowConversationSync'
import { ensureAgentRun } from '../../features/agentRun/agentEventReducer'
import {
  AccountAuthContext,
  type AccountAuthContextValue
} from '../../features/auth/AccountAuthContext'

const mocks = vi.hoisted(() => ({
  load: vi.fn(),
  save: vi.fn(),
  request: vi.fn(),
  listeners: new Set<(snapshot: WorkflowRuntimeSnapshot) => void>()
}))
vi.mock('../../host/hostClient', () => ({
  hostClient: {
    agent: {
      onWorkflowRuntimeChanged: (listener: (snapshot: WorkflowRuntimeSnapshot) => void) => {
        mocks.listeners.add(listener)
        return () => mocks.listeners.delete(listener)
      }
    }
  }
}))
vi.mock('../../features/storage/storageClient', () => ({ loadConversation: mocks.load }))
vi.mock('../../features/workflows/workflowClient', () => ({ requestWorkflows: mocks.request }))

const conversation = (): ChatConversation => ({
  id: 'chat',
  title: 'Chat',
  modelId: 'model',
  projectId: null,
  createdAt: 1,
  updatedAt: 1,
  messages: [
    {
      id: 'assistant',
      role: 'assistant',
      content: 'Live streamed text',
      createdAt: 1,
      status: 'pending',
      agentRun: ensureAgentRun(undefined, 'run', 'running')
    }
  ]
})
const snapshot = (status: 'claimed' | 'applied' = 'applied'): WorkflowRuntimeSnapshot => ({
  instanceId: 'workflow',
  sequence: status === 'applied' ? 2 : 1,
  inputs: [
    {
      id: 'input',
      instanceId: 'workflow',
      nodeId: 'receiver',
      conversationId: 'chat',
      executionVersion: 'v1',
      content: 'batch',
      messages: [],
      mailStatus: 'processing',
      status,
      runId: 'run',
      deliveryId: 'delivery',
      createdAt: 2,
      error: null
    }
  ],
  events: []
})
function Harness({
  visibleId = null,
  hydrated = true,
  initialChats
}: {
  visibleId?: string | null
  hydrated?: boolean
  initialChats?: ChatConversation[]
}) {
  const [conversations, setConversations] = useState(
    initialChats ?? (hydrated ? [conversation()] : [])
  )
  const conversationsRef = useRef(conversations)
  const pendingActionsHydratedRef = useRef(new Set(['chat']))
  const visibleConversationIdRef = useRef(visibleId)
  const update = useCallback((value: SetStateAction<ChatConversation[]>) => {
    const next = typeof value === 'function' ? value(conversationsRef.current) : value
    conversationsRef.current = next
    setConversations(next)
  }, [])
  useWorkflowConversationSync({
    conversationsRef,
    setConversations: update,
    pendingActionsHydratedRef,
    visibleConversationIdRef,
    enqueueConversationMetaSave: mocks.save
  })
  return (
    <>
      <button onClick={() => update([conversation()])}>Hydrate</button>
      <output
        data-unread={!!conversations[0]?.unreadAt}
        data-timeline={conversations[0]?.messages
          .flatMap((message) => message.agentRun?.timeline ?? [])
          .map((item) => item.id)
          .join('|')}
      >
        {conversations[0]?.messages.map((message) => `${message.id}:${message.content}`).join('|')}
      </output>
    </>
  )
}
beforeEach(() => {
  mocks.listeners.clear()
  mocks.load.mockReset()
  mocks.save.mockReset()
  mocks.request.mockReset().mockResolvedValue({ records: [], issues: [], instances: [] })
})

const summary = (sequence: number, ids = ['chat']): WorkflowRuntimeSnapshot => ({
  instanceId: 'workflow',
  sequence,
  inputs: [],
  events: [],
  summary: {
    pendingByNode: [],
    structureRevision: 0,
    conversationChanges: ids.map((conversationId) => ({ conversationId, sequence }))
  }
})

const signedIn = (userId: string, revision = 1): AccountAuthContextValue => ({
  state: {
    revision,
    status: 'signedIn',
    profile: {
      userId,
      displayName: userId,
      email: '',
      avatarDataUrl: null,
      occupation: '',
      organization: ''
    },
    error: null,
    remembered: false
  },
  loginRequested: false,
  requestLogin: vi.fn(),
  dismissLogin: vi.fn(),
  canStartTurn: () => true,
  logout: async () => ({ ok: true })
})

it('resets summary recovery once per account scope and ignores a previous account read', async () => {
  let finishPrevious!: (value: ChatConversation) => void
  mocks.load
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finishPrevious = resolve
        })
    )
    .mockResolvedValue({
      ...conversation(),
      messages: [{ id: 'new', role: 'user', content: 'New account result', createdAt: 2 }]
    })
  const renderAccount = (id: string, revision = 1) => (
    <AccountAuthContext.Provider value={signedIn(id, revision)}>
      <Harness />
    </AccountAuthContext.Provider>
  )
  const view = await render(renderAccount('a'))
  await act(() => mocks.listeners.forEach((listener) => listener(summary(700))))
  expect(mocks.load).toHaveBeenCalledTimes(1)
  await view.rerender(renderAccount('b'))
  await act(() => mocks.listeners.forEach((listener) => listener(summary(700))))
  await expect.element(view.getByRole('status')).toHaveTextContent('new:New account result')
  expect(mocks.load).toHaveBeenCalledTimes(2)
  const requestCount = mocks.request.mock.calls.length
  await view.rerender(renderAccount('b', 2))
  await act(() => mocks.listeners.forEach((listener) => listener(summary(700))))
  expect(mocks.request).toHaveBeenCalledTimes(requestCount)
  expect(mocks.load).toHaveBeenCalledTimes(2)
  await act(() =>
    finishPrevious({
      ...conversation(),
      messages: [{ id: 'old', role: 'user', content: 'Previous account result', createdAt: 3 }]
    })
  )
  await expect.element(view.getByRole('status')).toHaveTextContent('new:New account result')
  expect(mocks.listeners.size).toBe(1)
})

it('follows newer conversation changes across organizations without replaying an old binding', async () => {
  mocks.load.mockResolvedValue(conversation())
  await render(<Harness />)
  await act(() => mocks.listeners.forEach((listener) => listener(summary(800))))
  await expect.poll(() => mocks.load.mock.calls.length).toBe(1)
  await act(() =>
    mocks.listeners.forEach((listener) => listener({ ...summary(900), instanceId: 'new-org' }))
  )
  await expect.poll(() => mocks.load.mock.calls.length).toBe(2)
  await act(() => mocks.listeners.forEach((listener) => listener(summary(800))))
  expect(mocks.load).toHaveBeenCalledTimes(2)
})

it('coalesces changes during a pending conversation read into one final recovery', async () => {
  const finish: ((value: ChatConversation) => void)[] = []
  mocks.load.mockImplementation(
    () =>
      new Promise((resolve) => {
        finish.push(resolve)
      })
  )
  await render(<Harness />)
  await act(() => {
    for (const sequence of [10, 11, 12])
      mocks.listeners.forEach((listener) => listener(summary(sequence)))
  })
  expect(mocks.load).toHaveBeenCalledTimes(1)
  await act(() => finish[0](conversation()))
  await expect.poll(() => mocks.load.mock.calls.length).toBe(2)
  await act(() => finish[1](conversation()))
  await act(() => mocks.listeners.forEach((listener) => listener(summary(12))))
  expect(mocks.load).toHaveBeenCalledTimes(2)
})

it('recovers terminal and unread state from a summary beyond the recent event window', async () => {
  mocks.load.mockResolvedValue({ ...conversation(), unreadAt: 123 })
  const view = await render(<Harness />)
  const emit = (value: WorkflowRuntimeSnapshot) =>
    act(() => {
      mocks.listeners.forEach((listener) => listener(value))
    })
  await emit(summary(20))
  await expect.poll(() => mocks.load.mock.calls.length).toBe(1)
  mocks.load.mockResolvedValue({ ...conversation(), unreadAt: 999 })
  await emit(summary(2000))
  await expect.poll(() => mocks.load.mock.calls.length).toBe(2)
  await expect.element(view.getByRole('status')).toHaveAttribute('data-unread', 'true')
  await emit(summary(2000))
  await emit(summary(20))
  expect(mocks.load).toHaveBeenCalledTimes(2)
})

it('does not acknowledge unknown summary conversations before local metadata is hydrated', async () => {
  mocks.load.mockResolvedValue(conversation())
  const view = await render(<Harness hydrated={false} />)
  await act(() => mocks.listeners.forEach((listener) => listener(summary(600))))
  expect(mocks.load).not.toHaveBeenCalled()
  await view.getByRole('button', { name: 'Hydrate' }).click()
  await act(() => mocks.listeners.forEach((listener) => listener(summary(600))))
  await expect.poll(() => mocks.load.mock.calls.length).toBe(1)
})

it('bounds first recovery concurrency while retaining all known historical conversations', async () => {
  const chats = Array.from({ length: 8 }, (_, index) => ({
    ...conversation(),
    id: `chat-${index}`
  }))
  const pending = new Map<string, () => void>()
  mocks.load.mockImplementation(
    (id: string) =>
      new Promise((resolve) => {
        pending.set(id, () => resolve({ ...conversation(), id }))
      })
  )
  await render(<Harness initialChats={chats} />)
  const value = summary(800, [
    ...Array.from({ length: 5000 }, (_, index) => `unknown-${index}`),
    ...chats.map((chat) => chat.id)
  ])
  await act(() => mocks.listeners.forEach((listener) => listener(value)))
  expect(mocks.load).toHaveBeenCalledTimes(4)
  await act(async () => {
    for (const resolve of [...pending.values()]) resolve()
  })
  await expect.poll(() => mocks.load.mock.calls.length).toBe(8)
  await act(async () => {
    for (const resolve of [...pending.values()]) resolve()
  })
  await act(() => mocks.listeners.forEach((listener) => listener(value)))
  expect(mocks.load).toHaveBeenCalledTimes(8)
  expect(mocks.load.mock.calls.every(([id]) => String(id).startsWith('chat-'))).toBe(true)
})

it('retries a failed summary hydration through full-summary recovery without a cursor', async () => {
  mocks.request.mockImplementation(async (request) =>
    request.operation === 'listInstances'
      ? { records: [], issues: [], instances: [{ id: 'workflow' }] }
      : { records: [], issues: [], runtime: summary(700) }
  )
  mocks.load.mockRejectedValueOnce(new Error('busy')).mockResolvedValue(conversation())
  await render(<Harness initialChats={[{ ...conversation(), messages: [] }]} />)
  await expect.poll(() => mocks.load.mock.calls.length).toBe(1)
  window.dispatchEvent(new Event('focus'))
  await expect.poll(() => mocks.load.mock.calls.length).toBe(2)
  expect(mocks.request).toHaveBeenCalledWith({
    operation: 'runtimeSnapshot',
    instanceId: 'workflow',
    summaryOnly: true
  })
  expect(mocks.request.mock.calls.every(([request]) => !('afterSequence' in request))).toBe(true)
})
it('recovers a receipt that arrived before conversation metadata hydration', async () => {
  mocks.load.mockResolvedValue({
    ...conversation(),
    messages: [
      { id: 'delivery', role: 'user', content: 'Recovered organization input', createdAt: 2 }
    ]
  })
  const view = await render(<Harness hydrated={false} />)
  await act(() => {
    mocks.listeners.forEach((listener) => listener(snapshot()))
  })
  expect(mocks.load).not.toHaveBeenCalled()
  await view.getByRole('button', { name: 'Hydrate' }).click()
  await act(() => {
    mocks.listeners.forEach((listener) => listener(snapshot()))
  })
  await expect
    .element(view.getByRole('status'))
    .toHaveTextContent('delivery:Recovered organization input')
  expect(mocks.load).toHaveBeenCalledTimes(1)
})
it('reloads a terminal event even when the delivery receipt did not change', async () => {
  mocks.load.mockResolvedValue(conversation())
  const view = await render(<Harness visibleId="chat" />)
  await act(() => {
    mocks.listeners.forEach((listener) => listener(snapshot()))
  })
  await expect.poll(() => mocks.load.mock.calls.length).toBe(1)
  mocks.load.mockResolvedValue({ ...conversation(), unreadAt: 456 })
  const completed = {
    ...snapshot(),
    sequence: 3,
    events: [
      {
        sequence: 3,
        instanceId: 'workflow',
        inputId: 'input',
        messageId: null,
        sourceNodeId: null,
        targetNodeId: null,
        kind: 'run_completed',
        createdAt: 456
      }
    ]
  }
  await act(() => {
    mocks.listeners.forEach((listener) => listener(completed))
  })
  await expect.poll(() => mocks.load.mock.calls.length).toBe(2)
  await expect.element(view.getByRole('status')).toHaveAttribute('data-unread', 'false')
  expect(mocks.save).toHaveBeenCalledWith(expect.objectContaining({ unreadAt: null }))
})
it.each([null, 'chat'])(
  'keeps background deliveries unread and clears only the visible conversation: %s',
  async (visibleId) => {
    mocks.load.mockResolvedValue({ ...conversation(), unreadAt: 123 })
    const view = await render(<Harness visibleId={visibleId} />)
    await act(() => {
      mocks.listeners.forEach((listener) => listener(snapshot()))
    })
    await expect
      .element(view.getByRole('status'))
      .toHaveAttribute('data-unread', visibleId ? 'false' : 'true')
    if (visibleId)
      expect(mocks.save).toHaveBeenCalledWith(
        expect.objectContaining({ id: 'chat', unreadAt: null })
      )
    else expect(mocks.save).not.toHaveBeenCalled()
  }
)
it('loads a single batch bubble from Host notifications without erasing a live response or duplicating repeated receipts', async () => {
  const stored = conversation()
  stored.messages[0].content = 'Stale'
  stored.messages.push({
    id: 'delivery',
    role: 'user',
    content: 'Both source results in one message',
    createdAt: 2,
    workflowSource: {
      inputId: 'input',
      instanceId: 'workflow',
      workflowName: 'Review',
      sources: [
        {
          nodeId: 'source',
          nodeName: 'Source',
          conversationId: 'source-chat',
          conversationTitle: 'Source chat'
        }
      ]
    }
  })
  mocks.load.mockResolvedValue(stored)
  const view = await render(<Harness />)
  await act(() => {
    mocks.listeners.forEach((listener) => listener(snapshot()))
  })
  await expect
    .element(view.getByRole('status'))
    .toHaveTextContent('assistant:Live streamed text|delivery:Both source results in one message')
  await act(() => {
    mocks.listeners.forEach((listener) => listener(snapshot()))
  })
  expect(mocks.load).toHaveBeenCalledTimes(1)
  await view.unmount()
  expect(mocks.listeners.size).toBe(0)
})

it('recovers a same-turn delivery after a claimed snapshot and keeps it inside the live assistant', async () => {
  const claimed = conversation()
  claimed.messages[0].content = 'Older persisted narration'
  claimed.messages.push({
    id: 'delivery',
    role: 'user',
    content: 'Wrapped collaborator mail',
    createdAt: 2,
    workflowSource: {
      inputId: 'input',
      instanceId: 'workflow',
      workflowName: 'Review',
      sources: [
        {
          nodeId: 'source',
          nodeName: 'Boss',
          conversationId: 'source-chat',
          conversationTitle: 'Boss',
          content: 'Please continue'
        }
      ]
    }
  })
  mocks.load.mockResolvedValue(claimed)
  const view = await render(<Harness />)
  await act(() => {
    mocks.listeners.forEach((listener) => listener(snapshot('claimed')))
  })
  await expect
    .element(view.getByRole('status'))
    .toHaveTextContent('delivery:Wrapped collaborator mail')

  const applied = conversation()
  applied.messages[0].content = 'Older persisted narration'
  applied.messages[0].agentRun!.timeline = [
    {
      id: 'workflow-delivery-input',
      type: 'workflow_delivery',
      inputId: 'input',
      deliveryId: 'delivery',
      instanceId: 'workflow',
      workflowName: 'Review',
      content: 'Wrapped collaborator mail',
      createdAt: 2,
      traceSequence: 4,
      sources: [
        {
          nodeId: 'source',
          nodeName: 'Boss',
          conversationId: 'source-chat',
          conversationTitle: 'Boss',
          content: 'Please continue'
        }
      ]
    }
  ]
  mocks.load.mockResolvedValue(applied)
  await act(() => {
    mocks.listeners.forEach((listener) => listener(snapshot('applied')))
  })
  await expect.element(view.getByRole('status')).toHaveTextContent('assistant:Live streamed text')
  await expect.element(view.getByRole('status')).not.toHaveTextContent('delivery:')
  await expect
    .element(view.getByRole('status'))
    .toHaveAttribute('data-timeline', 'workflow-delivery-input')
  await act(() => {
    mocks.listeners.forEach((listener) => listener(snapshot('applied')))
  })
  expect(mocks.load).toHaveBeenCalledTimes(2)
})
