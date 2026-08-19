import type { IpcRenderer, IpcRendererEvent } from 'electron'
import { HOST_CHANNELS, type BrowserHostApi } from '@mycopilot/host-api'
import {
  parseBrowserSurfaceCommand,
  parseBrowserSurfaceReadyInput,
  parseBrowserSurfaceReadyOutput
} from '@mycopilot/protocol'

type BrowserIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>

/** Strict, transport-only bridge for the managed right-sidebar browser surface. */
export function createBrowserIpcBridge(ipcRenderer: BrowserIpcRenderer): BrowserHostApi {
  return {
    clearBrowsingData: () => ipcRenderer.invoke(HOST_CHANNELS.browser.clearBrowsingData),
    surfaceReady: (input) =>
      ipcRenderer
        .invoke(HOST_CHANNELS.browser.surfaceReady, parseBrowserSurfaceReadyInput(input))
        .then(parseBrowserSurfaceReadyOutput),
    onSurfaceCommand: (handler) => {
      const listener = (_event: IpcRendererEvent, value: unknown): void => {
        try {
          handler(parseBrowserSurfaceCommand(value))
        } catch {
          // Invalid Main-to-Renderer data is fail-closed and never reaches application state.
        }
      }
      ipcRenderer.on(HOST_CHANNELS.browser.surfaceCommand, listener)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.browser.surfaceCommand, listener)
    }
  }
}
