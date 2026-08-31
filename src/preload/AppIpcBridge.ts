import type { IpcRenderer, IpcRendererEvent } from 'electron'
import { HOST_CHANNELS, type AppWindowState, type HostApi } from '@mycopilot/host-api'

type AppIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener' | 'send'>

export function createAppIpcBridge(ipcRenderer: AppIpcRenderer): HostApi['app'] {
  return {
    getWindowState: () => ipcRenderer.invoke(HOST_CHANNELS.app.getWindowState),
    openExternal: (url) => ipcRenderer.invoke(HOST_CHANNELS.app.openExternal, url),
    onFlushBeforeQuit: (handler) => {
      const listener = (_event: IpcRendererEvent, requestId: unknown): void => {
        if (typeof requestId !== 'string' || requestId.length === 0) return
        void Promise.resolve()
          .then(handler)
          .catch((error) => console.error('Failed to flush Renderer state before quit', error))
          .finally(() => ipcRenderer.send(HOST_CHANNELS.app.flushBeforeQuitAck, requestId))
      }
      ipcRenderer.on(HOST_CHANNELS.app.flushBeforeQuit, listener)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.app.flushBeforeQuit, listener)
    },
    onWindowStateChange: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: AppWindowState): void => handler(payload)
      ipcRenderer.on(HOST_CHANNELS.app.windowStateChange, listener)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.app.windowStateChange, listener)
    },
    setNativeThemeSource: (themeSource) =>
      ipcRenderer.invoke(HOST_CHANNELS.app.setNativeThemeSource, themeSource),
    whenReady: () => ipcRenderer.invoke(HOST_CHANNELS.app.whenReady)
  }
}
