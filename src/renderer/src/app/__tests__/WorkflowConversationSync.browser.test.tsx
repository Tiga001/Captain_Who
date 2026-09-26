import { useCallback, useRef, useState, type SetStateAction } from 'react'
import { act } from 'react'
import { beforeEach, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { WorkflowRuntimeSnapshot } from '@mycopilot/protocol'
import type { ChatConversation } from '../../features/chat/chatTypes'
import { useWorkflowConversationSync } from '../useWorkflowConversationSync'
import { ensureAgentRun } from '../../features/agentRun/agentEventReducer'

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
      busyPolicy: 'inject',
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
  hydrated = true
}: {
  visibleId?: string | null
  hydrated?: boolean
}) {
  const [conversations, setConversations] = useState(hydrated ? [conversation()] : [])
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
      <output data-unread={!!conversations[0]?.unreadAt}>
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
it('recovers a receipt that arrived before conversation metadata hydration', async () => {
  mocks.load.mockResolvedValue({
    ...conversation(),
    messages: [{ id: 'delivery', role: 'user', content: 'Recovered workflow input', createdAt: 2 }]
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
    .toHaveTextContent('delivery:Recovered workflow input')
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
        flowIds: [],
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
