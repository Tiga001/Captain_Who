import type {
  NotificationEvent,
  NotificationResync,
  NotificationSettings
} from '@mycopilot/protocol'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-react'

interface Deferred<Value> {
  promise: Promise<Value>
  resolve(value: Value): void
}

function deferred<Value>(): Deferred<Value> {
  let resolve!: (value: Value) => void
  const promise = new Promise<Value>((finish) => {
    resolve = finish
  })
  return { promise, resolve }
}

const client = vi.hoisted(() => ({
  eventListeners: new Set<(event: NotificationEvent) => void>(),
  getNotificationSettings: vi.fn(),
  resyncListeners: new Set<(event: NotificationResync) => void>(),
  updateNotificationSettings: vi.fn()
}))

vi.mock('../notificationClient', () => ({
  getNotificationSettings: client.getNotificationSettings,
  hasNotificationHostApi: () => true,
  onNotificationEvent: (listener: (event: NotificationEvent) => void) => {
    client.eventListeners.add(listener)
    return () => client.eventListeners.delete(listener)
  },
  onNotificationResync: (listener: (event: NotificationResync) => void) => {
    client.resyncListeners.add(listener)
    return () => client.resyncListeners.delete(listener)
  },
  updateNotificationSettings: client.updateNotificationSettings
}))

const { useNotificationSettings } = await import('../useNotificationSettings')

type NotificationSettingsResult = ReturnType<typeof useNotificationSettings>

function settings(revision: number, enabled: boolean): NotificationSettings {
  return {
    schemaVersion: 1,
    enabled,
    soundEnabled: enabled,
    showTaskContent: enabled,
    humanCompletedEnabled: enabled,
    humanFailedEnabled: true,
    humanApprovalEnabled: true,
    humanCancelledEnabled: enabled,
    revision,
    updatedAt: revision
  }
}

function SettingsHarness({ onRender }: { onRender(value: NotificationSettingsResult): void }) {
  const value = useNotificationSettings()
  onRender(value)
  return <output>{`${value.status}:${value.settings?.revision ?? 'none'}`}</output>
}

describe('notification settings hook', () => {
  beforeEach(() => {
    client.eventListeners.clear()
    client.resyncListeners.clear()
    client.getNotificationSettings.mockReset()
    client.updateNotificationSettings.mockReset()
  })

  it('does not let an older settings response overwrite a newer refresh', async () => {
    const stale = deferred<NotificationSettings>()
    const fresh = deferred<NotificationSettings>()
    client.getNotificationSettings
      .mockReturnValueOnce(stale.promise)
      .mockReturnValueOnce(fresh.promise)

    let result!: NotificationSettingsResult
    const screen = await render(<SettingsHarness onRender={(value) => (result = value)} />)
    await expect.poll(() => client.getNotificationSettings.mock.calls.length).toBe(1)

    const latestRefresh = result.refresh()
    await expect.poll(() => client.getNotificationSettings.mock.calls.length).toBe(2)
    fresh.resolve(settings(2, false))
    await latestRefresh
    await expect.poll(() => result.settings?.revision).toBe(2)

    stale.resolve(settings(1, true))
    await stale.promise

    await expect.poll(() => result.settings?.revision).toBe(2)
    expect(result.settings).toMatchObject({ enabled: false, revision: 2 })
    expect(result.status).toBe('ready')
    expect(result.error).toBeNull()
    await screen.unmount()
  })

  it('refreshes settings after a settings event or resync without a list subscription', async () => {
    client.getNotificationSettings
      .mockResolvedValueOnce(settings(1, true))
      .mockResolvedValueOnce(settings(2, false))
      .mockResolvedValueOnce(settings(3, true))

    let result!: NotificationSettingsResult
    const screen = await render(<SettingsHarness onRender={(value) => (result = value)} />)
    await expect.poll(() => result.settings?.revision).toBe(1)

    for (const listener of client.eventListeners) {
      listener({
        schemaVersion: 1,
        sequence: 1,
        eventId: 'settings-event',
        kind: 'settings_updated',
        notificationId: null,
        batchId: null,
        resourceRevision: 2,
        occurredAt: 2
      })
    }
    await expect.poll(() => result.settings?.revision).toBe(2)

    for (const listener of client.resyncListeners) {
      listener({
        schemaVersion: 1,
        reason: 'core_started',
        lastSequence: 2,
        occurredAt: 3
      })
    }
    await expect.poll(() => result.settings?.revision).toBe(3)
    await screen.unmount()
  })
})
