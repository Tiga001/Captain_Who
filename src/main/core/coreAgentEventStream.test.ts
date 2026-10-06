import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { AgentEvent, AgentObserverEventEnvelope } from '@mycopilot/protocol'

const parsers = vi.hoisted(() => ({ child: vi.fn(), ordinary: vi.fn(), observer: vi.fn() }))
vi.mock('@mycopilot/protocol', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@mycopilot/protocol')>()
  parsers.child.mockImplementation(actual.parseAgentChildEventEnvelope)
  parsers.ordinary.mockImplementation(actual.parseAgentEventForHost)
  parsers.observer.mockImplementation(actual.parseAgentObserverEventEnvelope)
  return {
    ...actual,
    parseAgentChildEventEnvelope: parsers.child,
    parseAgentEventForHost: parsers.ordinary,
    parseAgentObserverEventEnvelope: parsers.observer
  }
})

import { CoreAgentEventStream } from './coreAgentEventStream'

const childMethod = 'agent.collaboration.childEvent'
const observerMethod = 'agent.collaboration.observerEvent'
const ordinaryMethod = 'agent.event'

function envelope(event?: AgentEvent, sequence = 1): AgentObserverEventEnvelope {
  return {
    schemaVersion: 1,
    rootAgentId: 'root-agent',
    rootConversationId: 'root-conversation',
    agentId: 'child-agent',
    conversationId: 'child-conversation',
    runId: 'child-run',
    assistantMessageId: 'child-message',
    streamCursor: { generation: 'generation-a', sequence },
    event: event ?? {
      type: 'message_delta',
      runId: 'child-run',
      streamId: 'stream',
      delta: 'hello'
    }
  }
}

function harness() {
  const routes = new Map<string, Set<(value: unknown) => void>>()
  const onNotification = vi.fn((method: string, listener: (value: unknown) => void) => {
    const handlers = routes.get(method) ?? new Set()
    handlers.add(listener)
    routes.set(method, handlers)
    return () => handlers.delete(listener)
  })
  return {
    stream: new CoreAgentEventStream({ onNotification }),
    onNotification,
    emit: (method: string, value: unknown) => {
      for (const listener of routes.get(method) ?? []) listener(value)
    },
    count: (method: string) => routes.get(method)?.size ?? 0
  }
}

describe('single child event transport fan-out', () => {
  beforeEach(() => vi.clearAllMocks())
  afterEach(() => vi.restoreAllMocks())

  it('validates one physical child event once and forwards existing shapes to every consumer in order', () => {
    const { stream, emit, onNotification } = harness()
    const order: string[] = []
    const ordinary = vi.fn<(event: AgentEvent) => number>(() => order.push('ordinary'))
    const tray = vi.fn<(event: AgentEvent) => number>(() => order.push('tray'))
    const observer = vi.fn<(event: AgentObserverEventEnvelope) => number>(() =>
      order.push('observer')
    )
    const secondObserver = vi.fn<(event: AgentObserverEventEnvelope) => number>(() =>
      order.push('second observer')
    )
    stream.onAgentEvent(ordinary)
    stream.onAgentEvent(tray)
    stream.onObserverEvent(observer)
    stream.onObserverEvent(secondObserver)
    const input = envelope()
    emit(childMethod, input)

    expect(onNotification).toHaveBeenCalledTimes(3)
    expect(parsers.child).toHaveBeenCalledExactlyOnceWith(input)
    expect(parsers.ordinary).not.toHaveBeenCalled()
    expect(parsers.observer).not.toHaveBeenCalled()
    expect(ordinary).toHaveBeenCalledExactlyOnceWith(input.event)
    expect(observer).toHaveBeenCalledExactlyOnceWith(input)
    expect(order).toEqual(['ordinary', 'tray', 'observer', 'second observer'])
    expect(ordinary.mock.calls[0][0]).not.toHaveProperty('rootAgentId')
    expect(observer.mock.calls[0][0]).toHaveProperty('rootConversationId', 'root-conversation')
  })

  it('keeps legacy ordinary and observer routes independent without duplicate fan-out', () => {
    const { stream, emit } = harness()
    const ordinary = vi.fn()
    const observer = vi.fn()
    stream.onAgentEvent(ordinary)
    stream.onObserverEvent(observer)
    const legacy = envelope()
    delete legacy.streamCursor
    emit(ordinaryMethod, legacy.event)
    expect(ordinary).toHaveBeenCalledTimes(1)
    expect(observer).not.toHaveBeenCalled()
    emit(observerMethod, legacy)
    expect(ordinary).toHaveBeenCalledTimes(1)
    expect(observer).toHaveBeenCalledExactlyOnceWith(legacy)
    expect(parsers.child).not.toHaveBeenCalled()
    expect(parsers.ordinary).toHaveBeenCalledTimes(1)
    expect(parsers.observer).toHaveBeenCalledTimes(1)
  })

  it('rejects missing or invalid cursors, mismatched identities and private payload fields on both routes', () => {
    const { stream, emit } = harness()
    const ordinary = vi.fn()
    const observer = vi.fn()
    stream.onAgentEvent(ordinary)
    stream.onObserverEvent(observer)
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    const input = envelope()
    const missingCursor = { ...input }
    delete missingCursor.streamCursor
    for (const invalid of [
      missingCursor,
      { ...input, streamCursor: { generation: 'private-generation-canary', sequence: -1 } },
      { ...input, event: { ...input.event, runId: 'private-other-run-canary' } },
      { ...input, event: { ...input.event, reasoningContent: 'private-content-canary' } },
      { ...input, rootConversationId: null },
      { ...input, identityProof: 'private-proof-canary' }
    ]) {
      emit(childMethod, invalid)
    }
    expect(ordinary).not.toHaveBeenCalled()
    expect(observer).not.toHaveBeenCalled()
    expect(warning).toHaveBeenCalled()
    expect(JSON.stringify(warning.mock.calls)).not.toContain('canary')
    expect(JSON.stringify(warning.mock.calls)).not.toContain('root-conversation')
  })

  it('preserves exact stream boundaries, late cursors and a restarted generation without buffering or deduplication', () => {
    const { stream, emit } = harness()
    const ordinary = vi.fn()
    const observer = vi.fn()
    stream.onAgentEvent(ordinary)
    stream.onObserverEvent(observer)
    const inputs = [
      envelope(
        { type: 'message_stream_started', runId: 'child-run', streamId: 'stream', attempt: 1 },
        1
      ),
      envelope(undefined, 2),
      envelope(
        {
          type: 'message_stream_reset',
          runId: 'child-run',
          streamId: 'stream',
          reason: 'retrying_model_request'
        },
        3
      ),
      envelope(
        { type: 'message_stream_started', runId: 'child-run', streamId: 'stream-2', attempt: 2 },
        4
      ),
      envelope(
        { type: 'message_delta', runId: 'child-run', streamId: 'stream-2', delta: 'final' },
        5
      ),
      envelope(
        {
          type: 'message_stream_committed',
          runId: 'child-run',
          streamId: 'stream-2',
          traceSequence: 7
        },
        6
      ),
      envelope({ type: 'done', runId: 'child-run', success: true, status: 'completed' }, 7),
      envelope(undefined, 2),
      { ...envelope(undefined, 1), streamCursor: { generation: 'generation-b', sequence: 1 } }
    ]
    for (const input of inputs) emit(childMethod, input)
    expect(ordinary.mock.calls.map(([event]) => event)).toEqual(inputs.map((input) => input.event))
    expect(observer.mock.calls.map(([event]) => event)).toEqual(inputs)
  })

  it('isolates throwing consumers without leaking errors or blocking another consumer or route', () => {
    const { stream, emit } = harness()
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    stream.onAgentEvent(() => {
      throw new Error('private-ordinary-error-canary')
    })
    const ordinary = vi.fn()
    stream.onAgentEvent(ordinary)
    stream.onObserverEvent(() => {
      throw new Error('private-observer-error-canary')
    })
    const observer = vi.fn()
    stream.onObserverEvent(observer)
    const input = envelope()
    expect(() => emit(childMethod, input)).not.toThrow()
    expect(ordinary).toHaveBeenCalledExactlyOnceWith(input.event)
    expect(observer).toHaveBeenCalledExactlyOnceWith(input)
    expect(JSON.stringify(warning.mock.calls)).not.toContain('canary')
  })

  it('releases unused wire subscriptions and reattaches once when consumers return', () => {
    const { stream, emit, count } = harness()
    const ordinary = vi.fn()
    const observer = vi.fn()
    const stopOrdinary = stream.onAgentEvent(ordinary)
    const stopDuplicate = stream.onAgentEvent(ordinary)
    const stopObserver = stream.onObserverEvent(observer)
    expect(count(childMethod)).toBe(1)
    emit(childMethod, envelope())
    expect(ordinary).toHaveBeenCalledTimes(2)
    stopOrdinary()
    emit(childMethod, envelope())
    expect(ordinary).toHaveBeenCalledTimes(3)
    stopDuplicate()
    expect(count(ordinaryMethod)).toBe(0)
    expect(count(childMethod)).toBe(1)
    stopObserver()
    expect(count(observerMethod)).toBe(0)
    expect(count(childMethod)).toBe(0)
    emit(childMethod, envelope())
    expect(ordinary).toHaveBeenCalledTimes(3)

    const stopNew = stream.onAgentEvent(ordinary)
    stopOrdinary()
    expect(count(childMethod)).toBe(1)
    emit(childMethod, envelope())
    expect(ordinary).toHaveBeenCalledTimes(4)
    stopNew()
    expect(count(childMethod)).toBe(0)
  })
})
