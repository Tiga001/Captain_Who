import { describe, expect, it, vi } from 'vitest'
import { createNotificationIpcBridge } from './NotificationIpcBridge'

describe('Notification IPC bridge', () => {
  it('uses only the standardized notification allowlist and validates push payloads', async () => {
    const invoke = vi.fn().mockResolvedValue({ ok: true, value: {} })
    const send = vi.fn()
    const listeners = new Map<string, (...args: unknown[]) => void>()
    const removeListener = vi.fn()
    const bridge = createNotificationIpcBridge({
      invoke,
      send,
      on: vi.fn((channel: string, listener: (...args: unknown[]) => void) => {
        listeners.set(channel, listener)
        return undefined as never
      }),
      removeListener
    })

    await bridge.setLocale('ja-JP')
    await bridge.markSeen({ schemaVersion: 1, target: { kind: 'batch', batchId: 'batch-1' } })
    await bridge.getSettings({ schemaVersion: 1 })
    await bridge.updateSettings({
      schemaVersion: 1,
      expectedRevision: 1,
      settings: {
        enabled: true,
        soundEnabled: true,
        showTaskContent: true,
        humanCompletedEnabled: true,
        humanFailedEnabled: true,
        humanApprovalEnabled: true,
        humanCancelledEnabled: true
      }
    })

    expect(invoke.mock.calls.map(([channel]) => channel)).toEqual([
      'host:notifications.setLocale',
      'host:notifications.markSeen',
      'host:notifications.settings.get',
      'host:notifications.settings.update'
    ])

    const onEvent = vi.fn()
    const onResync = vi.fn()
    const onOpen = vi.fn()
    const stopEvent = bridge.onEvent(onEvent)
    const stopResync = bridge.onResync(onResync)
    const stopOpen = bridge.onOpenRequested(onOpen)

    listeners.get('host:notifications.event')?.(
      {},
      {
        schemaVersion: 1,
        sequence: 1,
        eventId: 'event-1',
        kind: 'created',
        notificationId: 'notification-1',
        batchId: 'batch-1',
        resourceRevision: 1,
        occurredAt: 100
      }
    )
    listeners.get('host:notifications.event')?.({}, { schemaVersion: 1, privateState: true })
    listeners.get('host:notifications.resync')?.(
      {},
      {
        schemaVersion: 1,
        reason: 'core_started',
        lastSequence: 1,
        occurredAt: 101
      }
    )
    listeners.get('host:notifications.openRequested')?.(
      {},
      {
        schemaVersion: 1,
        batchId: 'batch-1',
        eventIds: ['notification-1'],
        destination: { kind: 'application' }
      }
    )

    expect(onEvent).toHaveBeenCalledOnce()
    expect(onResync).toHaveBeenCalledOnce()
    expect(onOpen).toHaveBeenCalledOnce()
    expect(send).toHaveBeenCalledTimes(2)
    expect(send).toHaveBeenNthCalledWith(1, 'host:notifications.resyncReady')
    expect(send).toHaveBeenNthCalledWith(2, 'host:notifications.openRequestedReady')

    stopEvent()
    stopResync()
    stopOpen()
    expect(removeListener).toHaveBeenCalledWith(
      'host:notifications.event',
      listeners.get('host:notifications.event')
    )
    expect(removeListener).toHaveBeenCalledWith(
      'host:notifications.resync',
      listeners.get('host:notifications.resync')
    )
    expect(removeListener).toHaveBeenCalledWith(
      'host:notifications.openRequested',
      listeners.get('host:notifications.openRequested')
    )
  })
})
