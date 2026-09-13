import { BrowserWindow, ipcMain, type IpcMainInvokeEvent } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import { createTrustedIpcMain } from '../ipc/trustedIpc'
import type { UpdateService } from './UpdateService'

export function registerUpdateIpc(
  service: UpdateService,
  trusted: (event: IpcMainInvokeEvent) => boolean
): () => void {
  const ipc = createTrustedIpcMain(trusted)
  const withoutInput =
    (action: () => unknown) =>
    (_event: IpcMainInvokeEvent, ...args: unknown[]) => {
      if (args.length) throw new Error('Update operation does not accept parameters')
      return action()
    }
  ipc.handle(HOST_CHANNELS.updates.getState, withoutInput(service.getState))
  ipc.handle(HOST_CHANNELS.updates.download, withoutInput(service.download))
  const unsubscribe = service.subscribe((state) => {
    for (const window of BrowserWindow.getAllWindows()) {
      if (window.isDestroyed() || window.webContents.isDestroyed()) continue
      // Broadcast only this public projection; actions remain protected by the trusted preload IPC.
      window.webContents.send(HOST_CHANNELS.updates.stateChanged, state)
    }
  })
  return () => {
    unsubscribe()
    ipcMain.removeHandler(HOST_CHANNELS.updates.getState)
    ipcMain.removeHandler(HOST_CHANNELS.updates.download)
  }
}
