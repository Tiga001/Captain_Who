import type { IpcRenderer } from 'electron'
import type { OfficeHostApi } from '@mycopilot/host-api'

export const OFFICE_GET_STATUS_CHANNEL = 'host:office.getStatus'

type OfficeIpcRenderer = Pick<IpcRenderer, 'invoke'>

/** Transport-only preload bridge; CoreServer owns response validation. */
export function createOfficeIpcBridge(ipcRenderer: OfficeIpcRenderer): OfficeHostApi {
  return {
    getStatus: () => ipcRenderer.invoke(OFFICE_GET_STATUS_CHANNEL)
  }
}
