import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('electron', () => ({
  Notification: class {
    static isSupported(): boolean {
      return false
    }
  }
}))

import { AutomationNotificationCoordinator } from '../automation/automationNotificationCoordinator'

function delivery() {
  return {
    schemaVersion: 1 as const,
    notificationId: 'notification-1',
    automationId: 'automation-1',
    runId: 'run-1',
    kind: 'run_result' as const,
    title: 'Daily brief completed',
    body: 'The scheduled task finished.',
    conversationId: 'conversation-1',
    userMessageId: 'message-user-1',
    assistantMessageId: 'message-assistant-1',
    createdAt: 100
  }
}

function nativeNotification() {
  const listeners = new Map<string, () => void>()
  const result = {
    listeners,
    on: vi.fn(),
    show: vi.fn(),
    close: vi.fn()
  }
  result.on.mockImplementation((event: string, listener: () => void) => {
    listeners.set(event, listener)
    return result
  })
  result.show.mockImplementation(() => listeners.get('show')?.())
  return result
}

async function flush(): Promise<void> {
  for (let index = 0; index < 8; index += 1) await Promise.resolve()
}

function harness(
  overrides: Record<string, unknown> = {},
  coreOverrides: Record<string, unknown> = {}
) {
  const clearTimer = vi.fn()
  const setTimer = vi.fn(() => ({ unref: vi.fn() }))
  const claim = vi.fn().mockResolvedValue({
    schemaVersion: 1,
    claimToken: 'claim-1',
    notifications: [delivery()]
  })
  const acknowledge = vi.fn().mockResolvedValue({
    schemaVersion: 1,
    notificationId: 'notification-1',
    status: 'delivered',
    deliveredAt: 200
  })
  const validate = vi.fn().mockResolvedValue({
    schemaVersion: 1,
    notificationId: 'notification-1',
    notification: delivery()
  })
  const release = vi.fn()
  const notification = nativeNotification()
  const createNotification = vi.fn(() => notification)
  const onOpenRequested = vi.fn()
  const coordinator = new AutomationNotificationCoordinator({
    coreServer: {
      claimAutomationNotifications: claim,
      validateAutomationNotification: validate,
      acknowledgeAutomationNotification: acknowledge,
      releaseAutomationNotification: release,
      ...coreOverrides
    } as never,
    onOpenRequested,
    dependencies: {
      clearInterval: clearTimer as never,
      createClaimToken: () => 'claim-1',
      createNotification: createNotification as never,
      isNotificationSupported: () => true,
      now: () => 1_000,
      setInterval: setTimer as never,
      ...overrides
    }
  })
  return {
    acknowledge,
    claim,
    clearTimer,
    coordinator,
    createNotification,
    notification,
    onOpenRequested,
    release,
    setTimer,
    validate
  }
}

describe('Automation native notification coordinator', () => {
  beforeEach(() => vi.restoreAllMocks())

  it('claims on startup, displays, acknowledges, and opens the exact assistant message', async () => {
    const state = harness()
    await flush()

    expect(state.claim).toHaveBeenCalledWith({
      schemaVersion: 1,
      claimToken: 'claim-1',
      leaseDurationMs: 60_000,
      limit: 10
    })
    expect(state.createNotification).toHaveBeenCalledWith({
      title: delivery().title,
      body: delivery().body
    })
    expect(state.validate).toHaveBeenCalledWith({
      schemaVersion: 1,
      notificationId: 'notification-1',
      claimToken: 'claim-1'
    })
    expect(state.notification.show).toHaveBeenCalledOnce()
    expect(state.acknowledge).toHaveBeenCalledWith({
      schemaVersion: 1,
      notificationId: 'notification-1',
      claimToken: 'claim-1'
    })

    state.notification.listeners.get('click')?.()
    expect(state.onOpenRequested).toHaveBeenCalledWith({
      schemaVersion: 1,
      automationId: 'automation-1',
      runId: 'run-1',
      destination: {
        kind: 'conversation',
        conversationId: 'conversation-1',
        messageId: 'message-assistant-1'
      }
    })

    state.coordinator.stop()
    expect(state.clearTimer).toHaveBeenCalledOnce()
    expect(state.notification.close).toHaveBeenCalledOnce()
  })

  it('acknowledges only after Electron confirms the native notification was shown', async () => {
    const deferredNotification = nativeNotification()
    deferredNotification.show.mockImplementation(() => undefined)
    const state = harness({ createNotification: () => deferredNotification })
    await flush()

    expect(deferredNotification.show).toHaveBeenCalledOnce()
    expect(state.acknowledge).not.toHaveBeenCalled()

    deferredNotification.listeners.get('show')?.()
    await flush()
    expect(state.acknowledge).toHaveBeenCalledOnce()
    state.coordinator.stop()
  })

  it('does not create or show a native notification when final Host validation is stale', async () => {
    const state = harness()
    state.validate.mockResolvedValue({
      schemaVersion: 1,
      notificationId: 'notification-1',
      notification: null
    })
    await flush()

    expect(state.validate).toHaveBeenCalledOnce()
    expect(state.createNotification).not.toHaveBeenCalled()
    expect(state.notification.show).not.toHaveBeenCalled()
    expect(state.acknowledge).not.toHaveBeenCalled()
    expect(state.release).not.toHaveBeenCalled()
    state.coordinator.stop()
  })

  it('fences native display when shutdown begins during final Host validation', async () => {
    let resolveValidation: (value: unknown) => void = () => {
      throw new Error('validation did not start')
    }
    const validate = vi.fn(
      () =>
        new Promise<unknown>((resolve) => {
          resolveValidation = resolve
        })
    )
    const state = harness({}, { validateAutomationNotification: validate })
    await flush()
    expect(validate).toHaveBeenCalledOnce()

    state.coordinator.stop()
    resolveValidation({
      schemaVersion: 1,
      notificationId: 'notification-1',
      notification: delivery()
    })
    await flush()

    expect(state.createNotification).not.toHaveBeenCalled()
    expect(state.notification.show).not.toHaveBeenCalled()
    expect(state.release).not.toHaveBeenCalled()
    expect(state.acknowledge).not.toHaveBeenCalled()
  })

  it('releases the durable claim when final Host validation fails transiently', async () => {
    const state = harness()
    state.validate.mockRejectedValue(new Error('core temporarily unavailable'))
    state.release.mockResolvedValue({
      schemaVersion: 1,
      notificationId: 'notification-1',
      status: 'pending',
      retryAt: 61_000
    })
    await flush()

    expect(state.createNotification).not.toHaveBeenCalled()
    expect(state.acknowledge).not.toHaveBeenCalled()
    expect(state.release).toHaveBeenCalledWith({
      schemaVersion: 1,
      notificationId: 'notification-1',
      claimToken: 'claim-1',
      retryAt: 61_000,
      errorCode: 'native_notification_failed'
    })
    state.coordinator.stop()
  })

  it('leaves outbox and attention untouched when native notifications are unavailable', async () => {
    const state = harness({ isNotificationSupported: () => false })
    await flush()
    expect(state.claim).not.toHaveBeenCalled()
    expect(state.createNotification).not.toHaveBeenCalled()
    state.coordinator.stop()
  })

  it('releases a failed native display with bounded retry metadata', async () => {
    const state = harness({
      createNotification: () => {
        throw new Error('native service unavailable')
      }
    })
    state.release.mockResolvedValue({
      schemaVersion: 1,
      notificationId: 'notification-1',
      status: 'pending',
      retryAt: 61_000
    })
    await flush()

    expect(state.acknowledge).not.toHaveBeenCalled()
    expect(state.release).toHaveBeenCalledWith({
      schemaVersion: 1,
      notificationId: 'notification-1',
      claimToken: 'claim-1',
      retryAt: 61_000,
      errorCode: 'native_notification_failed'
    })
    state.coordinator.stop()
  })

  it('releases a delivery when Electron reports an asynchronous native failure', async () => {
    const failedNotification = nativeNotification()
    failedNotification.show.mockImplementation(() => failedNotification.listeners.get('failed')?.())
    const state = harness({ createNotification: () => failedNotification })
    state.release.mockResolvedValue({
      schemaVersion: 1,
      notificationId: 'notification-1',
      status: 'pending',
      retryAt: 61_000
    })
    await flush()

    expect(state.acknowledge).not.toHaveBeenCalled()
    expect(state.release).toHaveBeenCalledWith({
      schemaVersion: 1,
      notificationId: 'notification-1',
      claimToken: 'claim-1',
      retryAt: 61_000,
      errorCode: 'native_notification_failed'
    })
    state.coordinator.stop()
  })

  it('retries only ACK after a shown delivery is reclaimed in the same process', async () => {
    const warning = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    const state = harness()
    state.acknowledge.mockRejectedValueOnce(new Error('connection lost')).mockResolvedValueOnce({
      schemaVersion: 1,
      notificationId: 'notification-1',
      status: 'delivered',
      deliveredAt: 300
    })
    await flush()
    state.coordinator.requestDrain()
    await flush()

    expect(state.claim).toHaveBeenCalledTimes(2)
    expect(state.createNotification).toHaveBeenCalledOnce()
    expect(state.acknowledge).toHaveBeenCalledTimes(2)
    expect(warning).toHaveBeenCalledWith(
      'Failed to acknowledge a displayed Automation notification'
    )
    state.coordinator.stop()
  })

  it('fences an in-flight drain and every later timer or event wake after shutdown', async () => {
    let resolveClaim: (value: unknown) => void = () => {
      throw new Error('claim did not start')
    }
    const claim = vi.fn(
      () =>
        new Promise<unknown>((resolve) => {
          resolveClaim = resolve
        })
    )
    const intervalHandlers: Array<() => void> = []
    const createNotification = vi.fn(() => nativeNotification())
    const coordinator = new AutomationNotificationCoordinator({
      coreServer: {
        claimAutomationNotifications: claim,
        validateAutomationNotification: vi.fn(),
        acknowledgeAutomationNotification: vi.fn(),
        releaseAutomationNotification: vi.fn()
      } as never,
      onOpenRequested: vi.fn(),
      dependencies: {
        clearInterval: vi.fn() as never,
        createClaimToken: () => 'shutdown-claim',
        createNotification: createNotification as never,
        isNotificationSupported: () => true,
        setInterval: ((handler: () => void) => {
          intervalHandlers.push(handler)
          return { unref: vi.fn() }
        }) as never
      }
    })
    await flush()
    expect(claim).toHaveBeenCalledOnce()

    coordinator.stop()
    resolveClaim({
      schemaVersion: 1,
      claimToken: 'shutdown-claim',
      notifications: [delivery()]
    })
    await flush()
    intervalHandlers[0]?.()
    coordinator.requestDrain()
    await flush()

    expect(claim).toHaveBeenCalledOnce()
    expect(createNotification).not.toHaveBeenCalled()
  })
})
