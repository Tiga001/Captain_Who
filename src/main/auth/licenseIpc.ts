import { app, BrowserWindow, ipcMain, type IpcMainInvokeEvent } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import { createTrustedIpcMain } from '../ipc/trustedIpc'
import type { LicenseService } from './LicenseService'
import type { LicenseManagementService } from './LicenseManagementService'

export function registerLicenseIpc(
  service: LicenseService,
  trusted: (event: IpcMainInvokeEvent) => boolean,
  management: LicenseManagementService
): () => void {
  const ipc = createTrustedIpcMain(trusted)
  ipc.handle(HOST_CHANNELS.license.getState, service.getState)
  ipc.handle(HOST_CHANNELS.license.refresh, service.refresh)
  ipc.handle(HOST_CHANNELS.license.openManagement, management.open)
  app.on('browser-window-blur', management.onBlur)
  app.on('browser-window-focus', management.onFocus)
  const unsubscribe = service.subscribe((state) => {
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed() && !window.webContents.isDestroyed())
        window.webContents.send(HOST_CHANNELS.license.stateChanged, state)
    }
  })
  return () => {
    unsubscribe()
    service.dispose()
    management.dispose()
    app.removeListener('browser-window-blur', management.onBlur)
    app.removeListener('browser-window-focus', management.onFocus)
    ipcMain.removeHandler(HOST_CHANNELS.license.getState)
    ipcMain.removeHandler(HOST_CHANNELS.license.refresh)
    ipcMain.removeHandler(HOST_CHANNELS.license.openManagement)
  }
}
