import type { IpcRenderer } from 'electron'
import { HOST_CHANNELS, type UpdateHostApi, type UpdateState } from '@mycopilot/host-api'

export function createUpdateIpcBridge(ipc: IpcRenderer): UpdateHostApi {
  return {
    getState: () => ipc.invoke(HOST_CHANNELS.updates.getState),
    download: () => ipc.invoke(HOST_CHANNELS.updates.download),
    onStateChanged: (handler) => {
      const listener = (_event: Electron.IpcRendererEvent, state: UpdateState): void =>
        handler(state)
      ipc.on(HOST_CHANNELS.updates.stateChanged, listener)
      return () => ipc.removeListener(HOST_CHANNELS.updates.stateChanged, listener)
    }
  }
}
