import type { IpcRenderer, IpcRendererEvent } from 'electron'
import { HOST_CHANNELS, type AppWindowState, type HostApi } from '@mycopilot/host-api'

type AppIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>

export function createAppIpcBridge(ipcRenderer: AppIpcRenderer): HostApi['app'] {
  return {
    getWindowState: () => ipcRenderer.invoke(HOST_CHANNELS.app.getWindowState),
    openExternal: (url) => ipcRenderer.invoke(HOST_CHANNELS.app.openExternal, url),
    onWindowStateChange: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: AppWindowState): void => handler(payload)
      ipcRenderer.on(HOST_CHANNELS.app.windowStateChange, listener)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.app.windowStateChange, listener)
    },
    setNativeThemeSource: (themeSource) =>
      ipcRenderer.invoke(HOST_CHANNELS.app.setNativeThemeSource, themeSource)
  }
}
