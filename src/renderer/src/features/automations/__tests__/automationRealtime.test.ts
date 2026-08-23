import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { AutomationEvent, AutomationResync } from '@mycopilot/protocol'

const transport = vi.hoisted(() => ({
  eventHandler: null as ((event: AutomationEvent) => void) | null,
  resyncHandler: null as ((event: AutomationResync) => void) | null,
  onEvent: vi.fn((handler: (event: AutomationEvent) => void) => {
    transport.eventHandler = handler
    return vi.fn()
  }),
  onResync: vi.fn((handler: (event: AutomationResync) => void) => {
    transport.resyncHandler = handler
    return vi.fn()
  })
}))

vi.mock('../automationClient', () => ({
  hasAutomationHostApi: () => true,
  onAutomationEvent: transport.onEvent,
  onAutomationResync: transport.onResync
}))

const { resetAutomationRealtimeForTests, subscribeAutomationRealtime } =
  await import('../automationRealtime')

function event(sequence: number): AutomationEvent {
  return {
    schemaVersion: 1,
    sequence,
    eventId: `event-${sequence}`,
    kind: 'updated',
    automationId: 'automation-1',
    runId: null,
    resourceRevision: sequence,
    occurredAt: sequence
  }
}

describe('automation realtime ordering', () => {
  beforeEach(() => {
    resetAutomationRealtimeForTests()
    transport.eventHandler = null
    transport.resyncHandler = null
    transport.onEvent.mockClear()
    transport.onResync.mockClear()
  })

  it('uses one Host subscription and drops duplicate/out-of-order events', () => {
    const first = vi.fn()
    const second = vi.fn()
    const stopFirst = subscribeAutomationRealtime(first)
    const stopSecond = subscribeAutomationRealtime(second)
    expect(transport.onEvent).toHaveBeenCalledOnce()
    expect(transport.onResync).toHaveBeenCalledOnce()

    transport.eventHandler?.(event(1))
    transport.eventHandler?.(event(1))
    transport.eventHandler?.(event(0))
    transport.eventHandler?.(event(3))

    expect(first).toHaveBeenCalledTimes(2)
    expect(second).toHaveBeenCalledTimes(2)
    expect(first.mock.calls[1]?.[0]).toMatchObject({ type: 'event', sequenceGap: true })
    stopFirst()
    stopSecond()
  })

  it('always distributes resync so consumers reload authoritative state', () => {
    const listener = vi.fn()
    const stop = subscribeAutomationRealtime(listener)
    transport.resyncHandler?.({
      schemaVersion: 1,
      reason: 'core_started',
      lastSequence: 8,
      occurredAt: 9
    })
    expect(listener).toHaveBeenCalledWith(expect.objectContaining({ type: 'resync' }))
    stop()
  })
})
