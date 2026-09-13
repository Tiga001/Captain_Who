import type { IpcRenderer } from 'electron'
import { HOST_CHANNELS, type LicenseHostApi, type LicenseState } from '@mycopilot/host-api'

export function createLicenseIpcBridge(ipc: IpcRenderer): LicenseHostApi {
  return {
    getState: () => ipc.invoke(HOST_CHANNELS.license.getState),
    refresh: () => ipc.invoke(HOST_CHANNELS.license.refresh),
    openManagement: () => ipc.invoke(HOST_CHANNELS.license.openManagement),
    onStateChanged: (handler) => {
      const listener = (_event: Electron.IpcRendererEvent, state: LicenseState): void =>
        handler(state)
      ipc.on(HOST_CHANNELS.license.stateChanged, listener)
      return () => {
        ipc.removeListener(HOST_CHANNELS.license.stateChanged, listener)
      }
    }
  }
}
