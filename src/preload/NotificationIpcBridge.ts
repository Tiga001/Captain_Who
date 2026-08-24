import type { IpcRenderer, IpcRendererEvent } from 'electron'
import { HOST_CHANNELS, type NotificationsHostApi } from '@mycopilot/host-api'
import {
  parseNotificationEvent,
  parseNotificationOpenRequest,
  parseNotificationResync
} from '@mycopilot/protocol'

type NotificationIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener' | 'send'>

/** Context-isolated notification transport. Main and Core remain delivery authorities. */
export function createNotificationIpcBridge(
  ipcRenderer: NotificationIpcRenderer
): NotificationsHostApi {
  return {
    setLocale: (language) => ipcRenderer.invoke(HOST_CHANNELS.notifications.setLocale, language),
    markSeen: (input) => ipcRenderer.invoke(HOST_CHANNELS.notifications.markSeen, input),
    getSettings: (input) => ipcRenderer.invoke(HOST_CHANNELS.notifications.getSettings, input),
    updateSettings: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.notifications.updateSettings, input),
    onEvent: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: unknown): void => {
        try {
          handler(parseNotificationEvent(payload))
        } catch {
          // Malformed Main-to-Renderer events cannot mutate Renderer notification state.
        }
      }
      ipcRenderer.on(HOST_CHANNELS.notifications.event, listener)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.notifications.event, listener)
    },
    onResync: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: unknown): void => {
        try {
          handler(parseNotificationResync(payload))
        } catch {
          // A malformed resync is ignored; Renderer retains its last authoritative snapshot.
        }
      }
      ipcRenderer.on(HOST_CHANNELS.notifications.resync, listener)
      ipcRenderer.send(HOST_CHANNELS.notifications.resyncReady)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.notifications.resync, listener)
    },
    onOpenRequested: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: unknown): void => {
        try {
          handler(parseNotificationOpenRequest(payload))
        } catch {
          // Invalid native-notification navigation must not change Renderer navigation.
        }
      }
      ipcRenderer.on(HOST_CHANNELS.notifications.openRequested, listener)
      // This handshake is intentionally separate from resync readiness. AppShell may subscribe
      // to resync before its navigation handler exists; Main must keep native-click navigation
      // queued until this exact listener is attached.
      ipcRenderer.send(HOST_CHANNELS.notifications.openRequestedReady)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.notifications.openRequested, listener)
    }
  }
}
