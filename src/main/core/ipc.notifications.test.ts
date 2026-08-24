import type { IpcMainInvokeEvent } from 'electron'
import { EventEmitter } from 'node:events'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { TrustedIpcMain } from '../ipc/trustedIpc'

const getAllWindows = vi.hoisted(() => vi.fn())
const fromWebContents = vi.hoisted(() => vi.fn())
const isNotificationSupported = vi.hoisted(() => vi.fn())
const createNativeNotification = vi.hoisted(() => vi.fn())

vi.mock('electron', () => ({
  app: { getLocale: () => 'zh-CN' },
  BrowserWindow: { getAllWindows, fromWebContents },
  Notification: class {
    static isSupported(): boolean {
      return isNotificationSupported()
    }

    constructor(options: unknown) {
      return createNativeNotification(options)
    }
  }
}))

import { registerNotificationIpc } from '../ipc/notificationIpc'

const invokeEvent = { sender: {} } as IpcMainInvokeEvent

function createTrustedIpc(): {
  handlers: Map<string, (...args: unknown[]) => unknown>
  listeners: Map<string, (...args: unknown[]) => unknown>
  ipc: TrustedIpcMain
} {
  const handlers = new Map<string, (...args: unknown[]) => unknown>()
  const listeners = new Map<string, (...args: unknown[]) => unknown>()
  return {
    handlers,
    listeners,
    ipc: {
      handle: (channel, handler) => handlers.set(channel, handler as never),
      on: (channel, handler) => listeners.set(channel, handler as never)
    }
  }
}

function notificationBatch() {
  return {
    schemaVersion: 1,
    batchId: 'batch-1',
    revision: 1,
    status: 'claimed',
    highestPriority: 'completed',
    counts: {
      completed: 1,
      failed: 0,
      cancelled: 0,
      approvalRequired: 0,
      importantUpdate: 0,
      configurationBlocked: 0
    },
    itemCount: 1,
    items: [
      {
        schemaVersion: 1,
        eventId: 'notification-1',
        batchId: 'batch-1',
        kind: 'task_completed',
        sourceKind: 'human_root',
        sourceId: 'run-1',
        runId: 'run-1',
        automationId: null,
        conversationId: 'conversation-1',
        userMessageId: 'message-user-1',
        assistantMessageId: 'message-assistant-1',
        approvalActionId: null,
        subjectKind: 'prompt_excerpt',
        subjectText: 'Prepare the report',
        priority: 'completed',
        resourceRevision: 1,
        seenAt: null,
        resolvedAt: null,
        occurredAt: 100
      }
    ],
    collectUntil: 100,
    replaceUntil: 60_000,
    notificationsEnabled: true,
    soundEnabled: true,
    showTaskContent: true,
    soundLevelPlayed: 'none',
    deliveredRevision: null,
    deliveredPriority: null,
    isUpdate: false,
    createdAt: 100,
    updatedAt: 100
  }
}

function createCore(overrides: Record<string, unknown> = {}) {
  return {
    onNotificationEvent: vi.fn(() => vi.fn()),
    onNotificationResync: vi.fn(() => vi.fn()),
    claimNotificationBatches: vi.fn((input: { claimToken: string }) =>
      Promise.resolve({ schemaVersion: 1, claimToken: input.claimToken, batches: [] })
    ),
    validateNotificationBatch: vi.fn(),
    acknowledgeNotificationBatch: vi.fn(),
    releaseNotificationBatch: vi.fn(),
    listNotifications: vi.fn().mockResolvedValue({
      schemaVersion: 1,
      items: [],
      nextCursor: null,
      unreadCount: 0,
      lastSequence: 0
    }),
    getNotificationSummary: vi.fn().mockResolvedValue({
      schemaVersion: 1,
      unreadCount: 0,
      unresolvedCount: 0,
      counts: {
        completed: 0,
        failed: 0,
        cancelled: 0,
        approvalRequired: 0,
        importantUpdate: 0,
        configurationBlocked: 0
      },
      latestOccurredAt: null,
      lastSequence: 0
    }),
    markNotificationSeen: vi.fn().mockResolvedValue({
      schemaVersion: 1,
      updatedCount: 0,
      seenAt: 101,
      summary: {
        schemaVersion: 1,
        unreadCount: 0,
        unresolvedCount: 0,
        counts: {
          completed: 0,
          failed: 0,
          cancelled: 0,
          approvalRequired: 0,
          importantUpdate: 0,
          configurationBlocked: 0
        },
        latestOccurredAt: null,
        lastSequence: 0
      }
    }),
    getNotificationSettings: vi.fn(),
    updateNotificationSettings: vi.fn(),
    ...overrides
  }
}

async function flush(): Promise<void> {
  for (let index = 0; index < 12; index += 1) await Promise.resolve()
}

describe('Main notification IPC', () => {
  beforeEach(() => {
    getAllWindows.mockReset().mockReturnValue([])
    fromWebContents.mockReset().mockReturnValue(null)
    isNotificationSupported.mockReset().mockReturnValue(false)
    createNativeNotification.mockReset()
  })

  it('registers the exact Renderer allowlist and strictly parses both sides', async () => {
    const trusted = createTrustedIpc()
    const core = createCore()
    const cleanup = registerNotificationIpc(trusted.ipc, core as never)

    expect([...trusted.handlers.keys()].sort()).toEqual(
      [
        HOST_CHANNELS.notifications.markSeen,
        HOST_CHANNELS.notifications.setLocale,
        HOST_CHANNELS.notifications.getSettings,
        HOST_CHANNELS.notifications.updateSettings
      ].sort()
    )
    await expect(
      trusted.handlers.get(HOST_CHANNELS.notifications.markSeen)?.(invokeEvent, {
        schemaVersion: 1,
        target: { kind: 'all' },
        privateState: true
      })
    ).resolves.toMatchObject({ ok: false })
    expect(core.markNotificationSeen).not.toHaveBeenCalled()

    await expect(
      trusted.handlers.get(HOST_CHANNELS.notifications.markSeen)?.(invokeEvent, {
        schemaVersion: 1,
        target: { kind: 'events', eventIds: ['notification-1'] }
      })
    ).resolves.toMatchObject({ ok: true, value: { updatedCount: 0 } })
    expect(core.markNotificationSeen).toHaveBeenCalledWith({
      schemaVersion: 1,
      target: { kind: 'events', eventIds: ['notification-1'] }
    })

    await expect(
      trusted.handlers.get(HOST_CHANNELS.notifications.setLocale)?.(invokeEvent, 'ja-JP')
    ).resolves.toBeUndefined()
    await expect(
      trusted.handlers.get(HOST_CHANNELS.notifications.setLocale)?.(
        invokeEvent,
        'unsupported-language'
      )
    ).rejects.toThrow('Invalid application language')
    cleanup()
  })

  it('validates and broadcasts realtime events and startup resync', () => {
    let eventListener: ((value: unknown) => void) | undefined
    let resyncListener: ((value: unknown) => void) | undefined
    const unsubscribeEvent = vi.fn()
    const unsubscribeResync = vi.fn()
    const send = vi.fn()
    getAllWindows.mockReturnValue([
      { isDestroyed: () => false, webContents: { isDestroyed: () => false, send } }
    ])
    const core = createCore({
      onNotificationEvent: vi.fn((listener) => {
        eventListener = listener
        return unsubscribeEvent
      }),
      onNotificationResync: vi.fn((listener) => {
        resyncListener = listener
        return unsubscribeResync
      })
    })
    const trusted = createTrustedIpc()
    const cleanup = registerNotificationIpc(trusted.ipc, core as never)
    const event = {
      schemaVersion: 1,
      sequence: 1,
      eventId: 'change-1',
      kind: 'created',
      notificationId: 'notification-1',
      batchId: 'batch-1',
      resourceRevision: 1,
      occurredAt: 100
    }
    const resync = {
      schemaVersion: 1,
      reason: 'core_started',
      lastSequence: 1,
      occurredAt: 101
    }
    eventListener?.(event)
    resyncListener?.(resync)

    expect(send).toHaveBeenCalledWith(HOST_CHANNELS.notifications.event, event)
    expect(send).toHaveBeenCalledWith(HOST_CHANNELS.notifications.resync, resync)
    cleanup()
    expect(unsubscribeEvent).toHaveBeenCalledOnce()
    expect(unsubscribeResync).toHaveBeenCalledOnce()
  })

  it('queues a native click until a trusted Renderer is ready, then focuses exact navigation', async () => {
    isNotificationSupported.mockReturnValue(true)
    const listeners = new Map<string, () => void>()
    const native = {
      on: vi.fn((name: string, listener: () => void) => {
        listeners.set(name, listener)
        return native
      }),
      show: vi.fn(() => listeners.get('show')?.()),
      close: vi.fn()
    }
    createNativeNotification.mockReturnValue(native)
    const batch = notificationBatch()
    const core = createCore({
      claimNotificationBatches: vi.fn((input: { claimToken: string }) =>
        Promise.resolve({ schemaVersion: 1, claimToken: input.claimToken, batches: [batch] })
      ),
      validateNotificationBatch: vi.fn().mockResolvedValue({
        schemaVersion: 1,
        batchId: 'batch-1',
        batch
      }),
      acknowledgeNotificationBatch: vi.fn().mockResolvedValue({
        schemaVersion: 1,
        batchId: 'batch-1',
        status: 'displayed',
        disposition: 'delivered',
        acknowledgedAt: 101
      }),
      listNotifications: vi.fn().mockResolvedValue({
        schemaVersion: 1,
        items: batch.items,
        nextCursor: null,
        unreadCount: 1,
        lastSequence: 1
      })
    })
    const trusted = createTrustedIpc()
    const cleanup = registerNotificationIpc(trusted.ipc, core as never)
    await flush()
    expect(native.show).toHaveBeenCalledOnce()
    listeners.get('click')?.()
    await flush()
    expect(core.markNotificationSeen).toHaveBeenCalledWith({
      schemaVersion: 1,
      target: { kind: 'events', eventIds: ['notification-1'] }
    })

    const send = vi.fn()
    const sender = Object.assign(new EventEmitter(), { isDestroyed: () => false, send })
    const owner = {
      isDestroyed: () => false,
      isMinimized: () => true,
      restore: vi.fn(),
      show: vi.fn(),
      focus: vi.fn()
    }
    fromWebContents.mockReturnValue(owner)
    // Resync readiness is deliberately insufficient: navigation remains queued until the exact
    // open-request listener is installed in AppShell.
    trusted.listeners.get(HOST_CHANNELS.notifications.resyncReady)?.({ sender })
    expect(send).not.toHaveBeenCalledWith(
      HOST_CHANNELS.notifications.openRequested,
      expect.anything()
    )
    trusted.listeners.get(HOST_CHANNELS.notifications.openRequestedReady)?.({ sender })

    expect(owner.restore).toHaveBeenCalledOnce()
    expect(owner.show).toHaveBeenCalledOnce()
    expect(owner.focus).toHaveBeenCalledOnce()
    expect(send).toHaveBeenCalledWith(HOST_CHANNELS.notifications.openRequested, {
      schemaVersion: 1,
      batchId: 'batch-1',
      eventIds: ['notification-1'],
      destination: {
        kind: 'conversation',
        conversationId: 'conversation-1',
        messageId: 'message-assistant-1',
        approvalActionId: null
      }
    })

    send.mockClear()
    sender.emit('did-start-loading')
    listeners.get('click')?.()
    await flush()
    expect(send).not.toHaveBeenCalled()
    send.mockImplementationOnce(() => {
      throw new Error('document replaced before delivery')
    })
    trusted.listeners.get(HOST_CHANNELS.notifications.openRequestedReady)?.({ sender })
    expect(send).toHaveBeenCalledOnce()
    send.mockImplementation(() => undefined)
    trusted.listeners.get(HOST_CHANNELS.notifications.openRequestedReady)?.({ sender })
    expect(send).toHaveBeenCalledWith(
      HOST_CHANNELS.notifications.openRequested,
      expect.objectContaining({ batchId: 'batch-1' })
    )
    cleanup()
  })
})
