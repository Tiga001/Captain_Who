import type { IpcRenderer, IpcRendererEvent } from 'electron'
import { HOST_CHANNELS, type BrowserHostApi } from '@mycopilot/host-api'
import {
  parseBrowserArtifactExportInput,
  parseBrowserArtifactExportOutput,
  parseBrowserArtifactReadInput,
  parseBrowserArtifactReadOutput,
  parseBrowserSurfaceActionInput,
  parseBrowserSurfaceCommand,
  parseBrowserSurfaceReadyInput,
  parseBrowserSurfaceReadyOutput,
  parseBrowserSurfaceSelectedInput,
  parseBrowserSurfaceSelectedOutput,
  parseBrowserSurfaceState,
  parseBrowserSurfaceStateInput
} from '@mycopilot/protocol'

type BrowserIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>

/** Strict, transport-only bridge for the managed right-sidebar browser surface. */
export function createBrowserIpcBridge(ipcRenderer: BrowserIpcRenderer): BrowserHostApi {
  return {
    clearBrowsingData: () => ipcRenderer.invoke(HOST_CHANNELS.browser.clearBrowsingData),
    exportArtifact: (input) =>
      ipcRenderer
        .invoke(HOST_CHANNELS.browser.artifactExport, parseBrowserArtifactExportInput(input))
        .then((result: unknown) => {
          if (!result || typeof result !== 'object' || !('ok' in result)) {
            throw new Error('Invalid Browser Artifact Host response')
          }
          if ((result as { ok?: unknown }).ok !== true) return result as never
          const envelope = result as { ok: true; value: unknown }
          return { ok: true, value: parseBrowserArtifactExportOutput(envelope.value) }
        }),
    readArtifactPreview: (input) =>
      ipcRenderer
        .invoke(HOST_CHANNELS.browser.artifactReadPreview, parseBrowserArtifactReadInput(input))
        .then((result: unknown) => {
          if (!result || typeof result !== 'object' || !('ok' in result)) {
            throw new Error('Invalid Browser Artifact Host response')
          }
          if ((result as { ok?: unknown }).ok !== true) return result as never
          const envelope = result as { ok: true; value: unknown }
          return { ok: true, value: parseBrowserArtifactReadOutput(envelope.value) }
        }),
    surfaceReady: (input) =>
      ipcRenderer
        .invoke(HOST_CHANNELS.browser.surfaceReady, parseBrowserSurfaceReadyInput(input))
        .then(parseBrowserSurfaceReadyOutput),
    surfaceSelected: (input) =>
      ipcRenderer
        .invoke(HOST_CHANNELS.browser.surfaceSelected, parseBrowserSurfaceSelectedInput(input))
        .then(parseBrowserSurfaceSelectedOutput),
    surfaceAction: (input) =>
      ipcRenderer
        .invoke(HOST_CHANNELS.browser.surfaceAction, parseBrowserSurfaceActionInput(input))
        .then(parseBrowserSurfaceState),
    surfaceState: (input) =>
      ipcRenderer
        .invoke(HOST_CHANNELS.browser.surfaceState, parseBrowserSurfaceStateInput(input))
        .then(parseBrowserSurfaceState),
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
    },
    onSurfaceState: (handler) => {
      const listener = (_event: IpcRendererEvent, value: unknown): void => {
        try {
          handler(parseBrowserSurfaceState(value))
        } catch {
          // Invalid Main-to-Renderer data is fail-closed and never reaches application state.
        }
      }
      ipcRenderer.on(HOST_CHANNELS.browser.surfaceStateChanged, listener)
      return () => ipcRenderer.removeListener(HOST_CHANNELS.browser.surfaceStateChanged, listener)
    }
  }
}
