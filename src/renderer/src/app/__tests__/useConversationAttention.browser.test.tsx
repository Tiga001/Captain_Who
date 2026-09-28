import { act } from 'react'
import { beforeEach, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { HostInvocationResult } from '@mycopilot/host-api'
import type {
  AgentEvent,
  HumanInteractionAttentionSnapshot,
  HumanInteractionRequestSnapshot
} from '@mycopilot/protocol'
import type { ChatConversation } from '../../features/chat/chatTypes'
import { useConversationAttention } from '../../features/chat/useConversationAttention'
import {
  deferred,
  question,
  submitted
} from '../../features/humanInteraction/__tests__/humanInteractionFixtures'

const service = vi.hoisted(() => ({
  attention: vi.fn(),
  list: vi.fn(),
  listeners: new Set<(request: HumanInteractionRequestSnapshot) => void>(),
  agents: new Set<(event: AgentEvent) => void>(),
  resync: new Set<() => void>()
}))
vi.mock('../../host/hostClient', () => ({
  hostClient: {
    agent: {
      onEvent: (listener: (event: AgentEvent) => void) => {
        service.agents.add(listener)
        return () => service.agents.delete(listener)
      }
    },
    humanInteraction: {
      getAttention: service.attention,
      listRequests: service.list,
      onRequestChanged: (listener: (request: HumanInteractionRequestSnapshot) => void) => {
        service.listeners.add(listener)
        return () => service.listeners.delete(listener)
      },
      onResync: (listener: () => void) => {
        service.resync.add(listener)
        return () => service.resync.delete(listener)
      }
    }
  }
}))
const conversation = (id = 'chat', patch: Partial<ChatConversation> = {}): ChatConversation => ({
  id,
  projectId: null,
  modelId: null,
  title: id,
  messages: [],
  messagesLoaded: false,
  createdAt: 1,
  updatedAt: 1,
  ...patch
})
const snapshot = (
  requests: HumanInteractionRequestSnapshot[] = [],
  requestSequence = Math.max(0, ...requests.map((r) => r.sequence)),
  approvals: string[] = []
): HostInvocationResult<HumanInteractionAttentionSnapshot> => ({
  ok: true,
  value: {
    requestSequence,
    requests: requests
      .filter((r) => r.status === 'open')
      .map(({ requestId, conversationId, sequence, revision }) => ({
        requestId,
        conversationId,
        sequence,
        revision
      })),
    approvalConversationIds: approvals
  }
})
const emit = (request: HumanInteractionRequestSnapshot) =>
  service.listeners.forEach((listener) => listener(request))
function Harness({ conversations }: { conversations: ChatConversation[] }) {
  return (
    <output data-testid="attention">
      {JSON.stringify(useConversationAttention(conversations))}
    </output>
  )
}
const value = () => JSON.parse(document.querySelector('[data-testid="attention"]')!.textContent!)
beforeEach(() => {
  service.attention.mockReset().mockResolvedValue(snapshot())
  service.list.mockReset()
  service.listeners.clear()
  service.resync.clear()
  service.agents.clear()
})

it('recovers sparse questions and approvals in one read without loading any conversation history', async () => {
  const open = question()
  service.attention.mockResolvedValue(snapshot([open], 100, ['chat']))
  await render(<Harness conversations={[conversation('chat', { unreadAt: 5 })]} />)
  await expect
    .poll(() => value().chat)
    .toEqual({ waitingApproval: true, waitingAnswer: true, unread: true })
  expect(service.attention).toHaveBeenCalledExactlyOnceWith({})
  expect(service.list).not.toHaveBeenCalled()
  service.attention.mockResolvedValue(snapshot([], 100, ['chat']))
  await act(() => emit(submitted(open)))
  await expect
    .poll(() => value().chat)
    .toEqual({ waitingApproval: true, waitingAnswer: false, unread: true })
})

it.each(['submitted', 'ignored', 'cancelled'] as const)(
  'clears %s immediately and never reopens from stale notifications or an in-flight snapshot',
  async (status) => {
    const open = question()
    service.attention.mockResolvedValueOnce(snapshot([open]))
    const stale = deferred<HostInvocationResult<HumanInteractionAttentionSnapshot>>()
    service.attention.mockReturnValueOnce(stale.promise).mockResolvedValue(snapshot([], 1))
    await render(<Harness conversations={[conversation()]} />)
    await expect.poll(() => value().chat.waitingAnswer).toBe(true)
    await act(() => window.dispatchEvent(new Event('focus')))
    await expect.poll(() => service.attention.mock.calls.length).toBe(2)
    const settled = submitted(open)
    const terminal: HumanInteractionRequestSnapshot =
      status === 'cancelled'
        ? { ...open, status, revision: 1 }
        : status === 'ignored'
          ? {
              ...settled,
              status,
              response: { ...settled.response!, kind: 'ignored', answers: [] },
              delivery: null
            }
          : settled
    await act(() => {
      emit(terminal)
      emit(open)
    })
    expect(value().chat.waitingAnswer).toBe(false)
    stale.resolve(snapshot([open]))
    await expect.poll(() => service.attention.mock.calls.length).toBe(3)
    await act(() => emit(open))
    expect(value().chat.waitingAnswer).toBe(false)
  }
)

it('does not remove a newer question received while the global snapshot was in flight', async () => {
  const pending = deferred<HostInvocationResult<HumanInteractionAttentionSnapshot>>()
  service.attention.mockReturnValueOnce(pending.promise)
  await render(<Harness conversations={[conversation()]} />)
  await expect.poll(() => service.attention.mock.calls.length).toBe(1)
  const fresh = question('new', 2)
  await act(() => emit(fresh))
  pending.resolve(snapshot([], 1))
  await expect.poll(() => value().chat.waitingAnswer).toBe(true)
  service.attention.mockResolvedValue(snapshot([], 2))
  await act(() => emit(submitted(fresh)))
  expect(value().chat.waitingAnswer).toBe(false)
})

it('preserves attention after a failed read and recovers dropped events on focus, online and resync', async () => {
  service.attention
    .mockResolvedValueOnce(snapshot([question()]))
    .mockRejectedValueOnce(new Error('Disconnected'))
    .mockResolvedValue(snapshot([], 1))
  await render(<Harness conversations={[conversation()]} />)
  await expect.poll(() => value().chat.waitingAnswer).toBe(true)
  await act(() => window.dispatchEvent(new Event('focus')))
  await expect.poll(() => service.attention.mock.calls.length).toBe(2)
  expect(value().chat.waitingAnswer).toBe(true)
  await act(() => window.dispatchEvent(new Event('online')))
  await expect.poll(() => value().chat.waitingAnswer).toBe(false)
  service.attention.mockResolvedValue(snapshot([question('restored', 1)]))
  await act(() => service.resync.forEach((listener) => listener()))
  await expect.poll(() => value().chat.waitingAnswer).toBe(true)
})

it('uses one coalesced request for 1000 empty conversations and never refetches on token updates', async () => {
  const chats = Array.from({ length: 1000 }, (_, index) => conversation(`c${index}`))
  const screen = await render(
    <Harness conversations={[...chats, conversation('archived', { archivedAt: 1 })]} />
  )
  await expect.poll(() => service.attention.mock.calls.length).toBe(1)
  expect(service.list).not.toHaveBeenCalled()
  await screen.rerender(
    <Harness
      conversations={chats.map((item) => ({
        ...item,
        updatedAt: 2,
        messages: [
          { id: 'reply', role: 'assistant', content: 'token', createdAt: 1, status: 'pending' }
        ]
      }))}
    />
  )
  expect(service.attention).toHaveBeenCalledTimes(1)
  await act(() => {
    window.dispatchEvent(new Event('focus'))
    window.dispatchEvent(new Event('online'))
    document.dispatchEvent(new Event('visibilitychange'))
  })
  await expect.poll(() => service.attention.mock.calls.length).toBe(2)
  await screen.unmount()
  expect(service.listeners.size).toBe(0)
  expect(service.agents.size).toBe(0)
  expect(service.resync.size).toBe(0)
})

it('does not let an older approval read undo a live state transition and ignores token events', async () => {
  service.attention.mockResolvedValueOnce(snapshot([], 0, ['chat']))
  const stale = deferred<HostInvocationResult<HumanInteractionAttentionSnapshot>>()
  service.attention.mockReturnValueOnce(stale.promise).mockResolvedValue(snapshot())
  await render(<Harness conversations={[conversation()]} />)
  await expect.poll(() => value().chat.waitingApproval).toBe(true)
  await act(() =>
    service.agents.forEach((listener) =>
      listener({ type: 'message_delta', runId: 'run', delta: 'token' })
    )
  )
  expect(service.attention).toHaveBeenCalledTimes(1)
  await act(() => window.dispatchEvent(new Event('focus')))
  await expect.poll(() => service.attention.mock.calls.length).toBe(2)
  await act(() =>
    service.agents.forEach((listener) =>
      listener({ type: 'started', runId: 'run', toolDefinitions: [] })
    )
  )
  stale.resolve(snapshot([], 0, ['chat']))
  await expect.poll(() => value().chat.waitingApproval).toBe(false)
})

it('prunes removed/archived conversations and reloads them when restored', async () => {
  service.attention.mockResolvedValue(snapshot([question()], 1, ['chat']))
  const screen = await render(<Harness conversations={[conversation()]} />)
  await expect.poll(() => value().chat.waitingAnswer).toBe(true)
  await screen.rerender(<Harness conversations={[conversation('chat', { archivedAt: 1 })]} />)
  expect(value().chat).toEqual({ waitingAnswer: false, waitingApproval: false, unread: false })
  service.attention.mockResolvedValue(snapshot([], 1))
  await screen.rerender(<Harness conversations={[conversation()]} />)
  await expect.poll(() => service.attention.mock.calls.length).toBe(2)
  await act(() => emit(question()))
  expect(value().chat.waitingAnswer).toBe(false)
})

it('rejects a pre-resync response and permits a smaller restored database watermark', async () => {
  const old = deferred<HostInvocationResult<HumanInteractionAttentionSnapshot>>()
  service.attention
    .mockReturnValueOnce(old.promise)
    .mockResolvedValue(snapshot([question('restored', 1)]))
  await render(<Harness conversations={[conversation()]} />)
  await expect.poll(() => service.attention.mock.calls.length).toBe(1)
  await act(() => service.resync.forEach((listener) => listener()))
  old.resolve(snapshot([], 100))
  await expect.poll(() => value().chat.waitingAnswer).toBe(true)
  expect(service.attention).toHaveBeenCalledTimes(2)
})
