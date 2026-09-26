import { act } from 'react'
import { beforeEach, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'
import type { HostInvocationResult } from '@mycopilot/host-api'
import type {
  HumanInteractionListOutput,
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
  list: vi.fn(),
  listeners: new Set<(request: HumanInteractionRequestSnapshot) => void>(),
  resync: new Set<() => void>()
}))
vi.mock('../../host/hostClient', () => ({
  hostClient: {
    humanInteraction: {
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
const page = (
  items: HumanInteractionRequestSnapshot[],
  nextCursor: string | null = null
): HostInvocationResult<HumanInteractionListOutput> => ({ ok: true, value: { items, nextCursor } })
const emit = (request: HumanInteractionRequestSnapshot) =>
  service.listeners.forEach((listener) => listener(request))
function Harness({ conversations }: { conversations: ChatConversation[] }) {
  const attention = useConversationAttention(conversations)
  return <output data-testid="attention">{JSON.stringify(attention)}</output>
}
const value = () => JSON.parse(document.querySelector('[data-testid="attention"]')!.textContent!)
beforeEach(() => {
  service.list.mockReset().mockResolvedValue(page([]))
  service.listeners.clear()
  service.resync.clear()
})

it('recovers paginated open questions for metadata-only chats and keeps async questions after completion', async () => {
  const open = question()
  service.list
    .mockResolvedValueOnce(page([submitted(question('old'))], 'next'))
    .mockResolvedValueOnce(page([open]))
  const screen = await render(<Harness conversations={[conversation('chat', { unreadAt: 5 })]} />)
  await expect
    .poll(() => value().chat)
    .toEqual({ waitingApproval: false, waitingAnswer: true, unread: true })
  expect(service.list).toHaveBeenNthCalledWith(2, {
    conversationId: 'chat',
    limit: 100,
    cursor: 'next'
  })
  await screen.rerender(
    <Harness
      conversations={[
        conversation('chat', {
          unreadAt: 5,
          messages: [
            { id: 'reply', role: 'assistant', content: 'Done', createdAt: 1, status: 'sent' }
          ]
        })
      ]}
    />
  )
  expect(value().chat.waitingAnswer).toBe(true)
  expect(service.list).toHaveBeenCalledTimes(2)
  await act(() => emit(submitted(open)))
  await expect.poll(() => value().chat.waitingAnswer).toBe(false)
  expect(value().chat.unread).toBe(true)
})

it.each(['ignored', 'cancelled'] as const)(
  'clears %s questions from events without reopening from older snapshots',
  async (status) => {
    const open = question()
    service.list.mockResolvedValue(page([open]))
    await render(<Harness conversations={[conversation()]} />)
    await expect.poll(() => value().chat.waitingAnswer).toBe(true)
    const terminal: HumanInteractionRequestSnapshot =
      status === 'cancelled'
        ? { ...open, status, revision: 1 }
        : {
            ...submitted(open),
            status,
            response: { ...submitted(open).response!, kind: 'ignored', answers: [] },
            delivery: null
          }
    await act(() => {
      emit(terminal)
      emit(open)
    })
    await expect.poll(() => value().chat.waitingAnswer).toBe(false)
    await act(() => service.resync.forEach((listener) => listener()))
    await expect.poll(() => service.list.mock.calls.length).toBe(2)
    expect(value().chat.waitingAnswer).toBe(false)
  }
)

it('does not let an in-flight scan undo a settlement or remove a newly received question', async () => {
  const pending = deferred<HostInvocationResult<HumanInteractionListOutput>>()
  service.list.mockReturnValueOnce(pending.promise)
  await render(<Harness conversations={[conversation()]} />)
  const old = question('old'),
    fresh = question('new', 2)
  await act(() => {
    emit(old)
    emit(submitted(old))
    emit(fresh)
  })
  pending.resolve(page([old]))
  await expect.poll(() => value().chat.waitingAnswer).toBe(true)
  await act(() => emit(submitted(fresh)))
  await expect.poll(() => value().chat.waitingAnswer).toBe(false)
})

it('preserves attention during failed reads and recovers missing records on focus and resync', async () => {
  service.list
    .mockResolvedValueOnce(page([question()]))
    .mockRejectedValueOnce(new Error('Disconnected'))
    .mockResolvedValue(page([]))
  await render(<Harness conversations={[conversation()]} />)
  await expect.poll(() => value().chat.waitingAnswer).toBe(true)
  await act(() => window.dispatchEvent(new Event('focus')))
  await expect.poll(() => service.list.mock.calls.length).toBe(2)
  expect(value().chat.waitingAnswer).toBe(true)
  await act(() => service.resync.forEach((listener) => listener()))
  await expect.poll(() => value().chat.waitingAnswer).toBe(false)
})

it('bounds recovery concurrency, skips archived conversations, and does not reload on message tokens', async () => {
  const pending = Array.from({ length: 7 }, () =>
    deferred<HostInvocationResult<HumanInteractionListOutput>>()
  )
  service.list.mockImplementation(
    ({ conversationId }) => pending[Number(conversationId.slice(1))].promise
  )
  const chats = Array.from({ length: 7 }, (_, index) => conversation(`c${index}`))
  const screen = await render(
    <Harness conversations={[...chats, conversation('archived', { archivedAt: 1 })]} />
  )
  expect(service.list).toHaveBeenCalledTimes(4)
  pending[0].resolve(page([]))
  await expect.poll(() => service.list.mock.calls.length).toBe(5)
  await screen.rerender(
    <Harness
      conversations={[
        ...chats.map((item) => ({
          ...item,
          updatedAt: 2,
          messages: [
            {
              id: 'reply',
              role: 'assistant' as const,
              content: 'token',
              createdAt: 1,
              status: 'pending' as const
            }
          ]
        })),
        conversation('archived', { archivedAt: 1 })
      ]}
    />
  )
  expect(service.list).toHaveBeenCalledTimes(5)
  pending.forEach((item) => item.resolve(page([])))
  await expect.poll(() => service.list.mock.calls.length).toBe(7)
  await screen.unmount()
  expect(service.listeners.size).toBe(0)
  expect(service.resync.size).toBe(0)
})
