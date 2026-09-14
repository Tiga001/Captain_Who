import type { IpcRenderer, IpcRendererEvent } from 'electron'
import { HOST_CHANNELS, type AppWindowState, type HostApi } from '@mycopilot/host-api'

type AppIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener' | 'send'>

export function createAppIpcBridge(ipcRenderer: AppIpcRenderer): HostApi['app'] {
  const quitFlushHandlers = new Set<() => void | Promise<void>>()
  let quitFlushListener: ((event: IpcRendererEvent, requestId: unknown) => void) | null = null

  return {
    getWindowState: () => ipcRenderer.invoke(HOST_CHANNELS.app.getWindowState),
    openDocumentation: () => ipcRenderer.invoke(HOST_CHANNELS.app.openDocumentation),
    openExternal: (url) => ipcRenderer.invoke(HOST_CHANNELS.app.openExternal, url),
    onFlushBeforeQuit: (handler) => {
      quitFlushHandlers.add(handler)
      if (!quitFlushListener) {
        quitFlushListener = (_event: IpcRendererEvent, requestId: unknown): void => {
          if (typeof requestId !== 'string' || requestId.length === 0) return
          const handlers = [...quitFlushHandlers]
          void Promise.all(
            handlers.map((flushHandler) =>
              Promise.resolve()
                .then(flushHandler)
                .catch((error) =>
                  console.error('Failed to flush Renderer state before quit', error)
                )
            )
          ).finally(() => ipcRenderer.send(HOST_CHANNELS.app.flushBeforeQuitAck, requestId))
        }
        ipcRenderer.on(HOST_CHANNELS.app.flushBeforeQuit, quitFlushListener)
      }
      return () => {
        quitFlushHandlers.delete(handler)
        if (quitFlushHandlers.size > 0 || !quitFlushListener) return
        ipcRenderer.removeListener(HOST_CHANNELS.app.flushBeforeQuit, quitFlushListener)
        quitFlushListener = null
      }
    },
    onWindowStateChange: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: AppWindowState): void => handler(payload)
      ipcRenderer.on(HOST_CHANNELS.app.windowStateChange, listener)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.app.windowStateChange, listener)
    },
    setNativeThemeSource: (themeSource) =>
      ipcRenderer.invoke(HOST_CHANNELS.app.setNativeThemeSource, themeSource),
    showAbout: () => ipcRenderer.invoke(HOST_CHANNELS.app.showAbout),
    whenReady: () => ipcRenderer.invoke(HOST_CHANNELS.app.whenReady)
  }
}
