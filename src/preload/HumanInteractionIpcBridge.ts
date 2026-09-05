import type { IpcRenderer, IpcRendererEvent } from 'electron'
import { HOST_CHANNELS, type HumanInteractionHostApi } from '@mycopilot/host-api'
import {
  parseHumanInteractionSettings,
  parseHumanInteractionRequestSnapshot
} from '@mycopilot/protocol'

type HumanInteractionIpcRenderer = Pick<IpcRenderer, 'invoke' | 'on' | 'removeListener'>

export function createHumanInteractionIpcBridge(
  ipcRenderer: HumanInteractionIpcRenderer
): HumanInteractionHostApi {
  return {
    getSettings: (input) => ipcRenderer.invoke(HOST_CHANNELS.humanInteraction.getSettings, input),
    updateSettings: (input) =>
      ipcRenderer.invoke(HOST_CHANNELS.humanInteraction.updateSettings, input),
    listRequests: (input) => ipcRenderer.invoke(HOST_CHANNELS.humanInteraction.listRequests, input),
    submit: (input) => ipcRenderer.invoke(HOST_CHANNELS.humanInteraction.submit, input),
    ignore: (input) => ipcRenderer.invoke(HOST_CHANNELS.humanInteraction.ignore, input),
    onSettingsChanged: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: unknown): void => {
        let settings: ReturnType<typeof parseHumanInteractionSettings>
        try {
          settings = parseHumanInteractionSettings(payload)
        } catch {
          return
        }
        handler(settings)
      }
      ipcRenderer.on(HOST_CHANNELS.humanInteraction.settingsChanged, listener)
      return () =>
        ipcRenderer.removeListener(HOST_CHANNELS.humanInteraction.settingsChanged, listener)
    },
    onRequestChanged: (handler) => {
      const listener = (_event: IpcRendererEvent, payload: unknown): void => {
        let request: ReturnType<typeof parseHumanInteractionRequestSnapshot>
        try {
          request = parseHumanInteractionRequestSnapshot(payload)
        } catch {
          return
        }
        handler(request)
      }
      ipcRenderer.on(HOST_CHANNELS.humanInteraction.requestChanged, listener)
      return () =>
        ipcRenderer.removeListener(HOST_CHANNELS.humanInteraction.requestChanged, listener)
    }
  }
}
