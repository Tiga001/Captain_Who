import type { IpcRenderer } from 'electron'
import { HOST_CHANNELS, type OfficeHostApi } from '@mycopilot/host-api'

export const OFFICE_GET_STATUS_CHANNEL = HOST_CHANNELS.office.getStatus

type OfficeIpcRenderer = Pick<IpcRenderer, 'invoke'>

/** Transport-only preload bridge; CoreServer owns response validation. */
export function createOfficeIpcBridge(ipcRenderer: OfficeIpcRenderer): OfficeHostApi {
  return {
    getStatus: () => ipcRenderer.invoke(OFFICE_GET_STATUS_CHANNEL)
  }
}
