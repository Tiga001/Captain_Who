import { afterEach, beforeEach, expect, it, vi } from 'vitest'
import type { HumanInteractionHostApi } from '@mycopilot/host-api'
import type {
  AgentEvent,
  HumanInteractionAttentionSnapshot,
  HumanInteractionRequestSnapshot
} from '@mycopilot/protocol'
import { ConversationAttentionStore } from '../conversationAttentionStore'
import { question } from '../../humanInteraction/__tests__/humanInteractionFixtures'

const snapshot = (approvals: string[] = []) => ({
  ok: true as const,
  value: {
    requestSequence: 0,
    requests: [],
    approvalConversationIds: approvals
  } satisfies HumanInteractionAttentionSnapshot
})

beforeEach(() => {
  vi.useFakeTimers()
  vi.stubGlobal('window', new EventTarget())
  vi.stubGlobal('document', Object.assign(new EventTarget(), { visibilityState: 'visible' }))
})
afterEach(() => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

function fixture() {
  const getAttention = vi.fn().mockResolvedValue(snapshot())
  let emitRequest!: (request: HumanInteractionRequestSnapshot) => void
  const api = {
    getAttention,
    onRequestChanged: (listener: typeof emitRequest) => {
      emitRequest = listener
      return () => {}
    },
    onResync: () => () => {}
  } as unknown as HumanInteractionHostApi
  const store = new ConversationAttentionStore(api)
  let emit!: (event: AgentEvent) => void
  const disconnect = store.connect((listener) => {
    emit = listener
    return () => {}
  })
  store.watch(['chat'])
  return { store, getAttention, emit, emitRequest, disconnect }
}

it('refreshes approval attention within a bounded time during continuous lifecycle events', async () => {
  const { store, getAttention, emit, disconnect } = fixture()
  try {
    await vi.advanceTimersByTimeAsync(0)
    expect(getAttention).toHaveBeenCalledTimes(1)
    getAttention.mockResolvedValue(snapshot(['chat']))
    for (let index = 0; index < 75; index++) {
      emit({ type: 'started', runId: `run-${index}`, toolDefinitions: [] })
      await vi.advanceTimersByTimeAsync(20)
    }
    expect(getAttention.mock.calls.length).toBeGreaterThan(1)
    expect(store.getSnapshot().chat.waitingApproval).toBe(true)
  } finally {
    disconnect()
  }
})

it('publishes a completed approval read during continuous invalidations when reads are slow', async () => {
  const { store, getAttention, emit, disconnect } = fixture()
  try {
    await vi.advanceTimersByTimeAsync(0)
    getAttention.mockImplementation(
      () => new Promise((resolve) => setTimeout(() => resolve(snapshot(['chat'])), 80))
    )
    for (let index = 0; index < 75; index++) {
      emit({ type: 'started', runId: `run-${index}`, toolDefinitions: [] })
      store.refresh()
      await vi.advanceTimersByTimeAsync(20)
    }
    expect(getAttention.mock.calls.length).toBeGreaterThan(5)
    expect(store.getSnapshot().chat.waitingApproval).toBe(true)
  } finally {
    disconnect()
  }
})

it('automatically retries a failed read while retaining the last successful projection', async () => {
  const { store, getAttention, disconnect } = fixture()
  try {
    getAttention.mockResolvedValueOnce(snapshot(['chat']))
    await vi.advanceTimersByTimeAsync(0)
    expect(store.getSnapshot().chat.waitingApproval).toBe(true)
    getAttention
      .mockRejectedValueOnce(new Error('temporary disconnect'))
      .mockResolvedValue(snapshot())
    store.refresh()
    await vi.advanceTimersByTimeAsync(0)
    expect(getAttention).toHaveBeenCalledTimes(2)
    expect(store.getSnapshot().chat.waitingApproval).toBe(true)
    await vi.advanceTimersByTimeAsync(249)
    expect(getAttention).toHaveBeenCalledTimes(2)
    await vi.advanceTimersByTimeAsync(1)
    expect(getAttention).toHaveBeenCalledTimes(3)
    expect(store.getSnapshot().chat.waitingApproval).toBe(false)
    getAttention.mockRejectedValue(new Error('offline'))
    store.refresh()
    await vi.advanceTimersByTimeAsync(0)
    disconnect()
    const callsAtDisconnect = getAttention.mock.calls.length
    await vi.advanceTimersByTimeAsync(60_000)
    expect(getAttention).toHaveBeenCalledTimes(callsAtDisconnect)
  } finally {
    disconnect()
  }
})

it('releases request and known-state caches when a thousand watched conversations are removed', async () => {
  const { store, emitRequest, disconnect } = fixture()
  try {
    const ids = Array.from({ length: 1000 }, (_, index) => `chat-${index}`)
    store.watch(ids)
    await vi.advanceTimersByTimeAsync(0)
    for (let index = 0; index < ids.length; index++) {
      emitRequest({ ...question(`q-${index}`, index + 1), conversationId: ids[index] })
    }
    expect(Object.keys(store.getSnapshot())).toHaveLength(1000)
    expect(Object.values(store.getSnapshot()).every((value) => value.waitingAnswer)).toBe(true)
    store.watch(['chat-5'])
    expect(Object.keys(store.getSnapshot())).toEqual(['chat-5'])
    expect(Reflect.get(store, 'requests').size).toBe(1)
    store.watch([])
    expect(store.getSnapshot()).toEqual({})
    for (const cache of ['requests', 'known', 'knownApprovals', 'approvals']) {
      expect(Reflect.get(store, cache).size).toBe(0)
    }
  } finally {
    disconnect()
  }
})
