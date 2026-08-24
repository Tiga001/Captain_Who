import { BrowserWindow, type WebContents } from 'electron'
import { captureHostInvocation, HOST_CHANNELS } from '@mycopilot/host-api'
import {
  NOTIFICATION_SCHEMA_VERSION,
  parseNotificationEvent,
  parseNotificationMarkSeenInput,
  parseNotificationMarkSeenOutput,
  parseNotificationOpenRequest,
  parseNotificationResync,
  parseNotificationSettingsGetInput,
  parseNotificationSettingsGetOutput,
  parseNotificationSettingsUpdateInput,
  parseNotificationSettingsUpdateOutput,
  type NotificationOpenRequest,
  type NotificationResync
} from '@mycopilot/protocol'
import type { CoreServer } from '../core/coreServer'
import { SystemNotificationCoordinator } from '../notifications/systemNotificationCoordinator'
import {
  createVolatileNotificationLocaleMirror,
  type NotificationLocaleMirror
} from '../notifications/notificationLocaleStore'
import type { TrustedIpcMain } from './trustedIpc'

const MAX_PENDING_OPEN_REQUESTS = 32

export interface NotificationIpcRegistration {
  (): void
  beginShutdown(): Promise<void>
  beginDelivery(): void
}

function broadcast(channel: string, payload: unknown): void {
  for (const window of BrowserWindow.getAllWindows()) {
    if (window.isDestroyed() || window.webContents.isDestroyed()) continue
    try {
      window.webContents.send(channel, payload)
    } catch {
      console.warn('Failed to broadcast a system notification event to a window')
    }
  }
}

export function registerNotificationIpc(
  ipcMain: TrustedIpcMain,
  coreServer: CoreServer,
  options: { localeMirror?: NotificationLocaleMirror; startPaused?: boolean } = {}
): NotificationIpcRegistration {
  const localeMirror = options.localeMirror ?? createVolatileNotificationLocaleMirror()
  let latestResync: NotificationResync | null = null
  const pendingOpenRequests: NotificationOpenRequest[] = []
  const readyRenderers: WebContents[] = []
  const readyRendererLifecycleCleanup = new Map<WebContents, () => void>()

  const removeReadyRenderer = (target: WebContents): void => {
    const index = readyRenderers.indexOf(target)
    if (index >= 0) readyRenderers.splice(index, 1)
    const cleanup = readyRendererLifecycleCleanup.get(target)
    readyRendererLifecycleCleanup.delete(target)
    cleanup?.()
  }

  const watchReadyRendererDocument = (target: WebContents): void => {
    removeReadyRenderer(target)
    readyRenderers.push(target)
    if (typeof target.once !== 'function' || typeof target.removeListener !== 'function') return
    const invalidate = (): void => removeReadyRenderer(target)
    target.once('did-start-loading', invalidate)
    target.once('render-process-gone', invalidate)
    target.once('destroyed', invalidate)
    readyRendererLifecycleCleanup.set(target, () => {
      target.removeListener('did-start-loading', invalidate)
      target.removeListener('render-process-gone', invalidate)
      target.removeListener('destroyed', invalidate)
    })
  }

  const sendOpenRequest = (value: NotificationOpenRequest): void => {
    const request = parseNotificationOpenRequest(value)
    while (readyRenderers.length > 0) {
      const target = readyRenderers.at(-1)
      if (!target || target.isDestroyed()) {
        if (target) removeReadyRenderer(target)
        else readyRenderers.pop()
        continue
      }
      const window = BrowserWindow.fromWebContents(target)
      if (window && !window.isDestroyed()) {
        try {
          if (window.isMinimized()) window.restore()
          window.show()
          window.focus()
        } catch {
          // The navigation event remains useful if native focus loses a window-close race.
        }
      }
      try {
        target.send(HOST_CHANNELS.notifications.openRequested, request)
        return
      } catch {
        removeReadyRenderer(target)
        console.warn('Failed to deliver system notification navigation to a window')
      }
    }
    pendingOpenRequests.push(request)
    if (pendingOpenRequests.length > MAX_PENDING_OPEN_REQUESTS) pendingOpenRequests.shift()
  }

  const coordinator = new SystemNotificationCoordinator({
    coreServer,
    startPaused: options.startPaused,
    dependencies: { getLocale: () => localeMirror.getLocale() },
    onOpenRequested: (request, eventIds) => {
      // A native click is a user interaction even when no Renderer is ready yet. Persist the
      // exact presented snapshot here. Marking the whole mutable batch could accidentally consume
      // a higher-priority event attached between the OS click and this transaction.
      void coreServer
        .markNotificationSeen({
          schemaVersion: NOTIFICATION_SCHEMA_VERSION,
          target: { kind: 'events', eventIds }
        })
        .catch(() => console.warn('Failed to mark a clicked system notification as seen'))
      sendOpenRequest(request)
    }
  })
  const unsubscribeEvent = coreServer.onNotificationEvent((value) => {
    const event = parseNotificationEvent(value)
    broadcast(HOST_CHANNELS.notifications.event, event)
    coordinator.handleNotificationEvent(event)
  })
  const unsubscribeResync = coreServer.onNotificationResync((value) => {
    latestResync = parseNotificationResync(value)
    broadcast(HOST_CHANNELS.notifications.resync, latestResync)
    coordinator.requestDrain()
  })

  ipcMain.on(HOST_CHANNELS.notifications.resyncReady, (event) => {
    if (latestResync !== null && !event.sender.isDestroyed()) {
      event.sender.send(HOST_CHANNELS.notifications.resync, latestResync)
    }
  })

  ipcMain.on(HOST_CHANNELS.notifications.openRequestedReady, (event) => {
    watchReadyRendererDocument(event.sender)
    while (
      pendingOpenRequests.length > 0 &&
      !event.sender.isDestroyed() &&
      readyRenderers.includes(event.sender)
    ) {
      const request = pendingOpenRequests.shift()
      if (request) sendOpenRequest(request)
    }
  })

  ipcMain.handle(HOST_CHANNELS.notifications.markSeen, (_event, input) =>
    captureHostInvocation(async () =>
      parseNotificationMarkSeenOutput(
        await coreServer.markNotificationSeen(parseNotificationMarkSeenInput(input))
      )
    )
  )
  ipcMain.handle(HOST_CHANNELS.notifications.setLocale, async (_event, language) => {
    await localeMirror.setLocale(language)
  })
  ipcMain.handle(HOST_CHANNELS.notifications.getSettings, (_event, input) =>
    captureHostInvocation(async () =>
      parseNotificationSettingsGetOutput(
        await coreServer.getNotificationSettings(parseNotificationSettingsGetInput(input))
      )
    )
  )
  ipcMain.handle(HOST_CHANNELS.notifications.updateSettings, (_event, input) =>
    captureHostInvocation(async () =>
      parseNotificationSettingsUpdateOutput(
        await coreServer.updateNotificationSettings(parseNotificationSettingsUpdateInput(input))
      )
    )
  )

  let disposed = false
  const beginShutdown = async (): Promise<void> => {
    coordinator.stop()
    await localeMirror.beginShutdown()
  }
  const dispose = (): void => {
    if (disposed) return
    disposed = true
    void beginShutdown()
    for (const renderer of [...readyRenderers]) removeReadyRenderer(renderer)
    pendingOpenRequests.length = 0
    unsubscribeEvent()
    unsubscribeResync()
  }
  dispose.beginShutdown = beginShutdown
  dispose.beginDelivery = (): void => coordinator.start()
  return dispose
}
