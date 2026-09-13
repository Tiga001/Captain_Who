import { BrowserWindow, ipcMain, shell, type IpcMainInvokeEvent } from 'electron'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import { createTrustedIpcMain } from '../ipc/trustedIpc'
import { ACCOUNT_PAGES } from './accountConfig'
import type { AuthService } from './AuthService'

export function registerAuthIpc(
  auth: AuthService,
  trusted: (event: IpcMainInvokeEvent) => boolean
): () => void {
  const ipc = createTrustedIpcMain(trusted)
  ipc.handle(HOST_CHANNELS.auth.getState, auth.getState)
  ipc.handle(HOST_CHANNELS.auth.restoreSession, auth.restoreSession)
  ipc.handle(HOST_CHANNELS.auth.login, (_event, input) => auth.login(input))
  ipc.handle(HOST_CHANNELS.auth.sendEmailCode, (_event, email) => auth.sendEmailCode(email))
  ipc.handle(HOST_CHANNELS.auth.verifyEmailCode, (_event, input) => auth.verifyEmailCode(input))
  ipc.handle(HOST_CHANNELS.auth.logout, auth.logout)
  ipc.handle(HOST_CHANNELS.auth.refreshProfile, () => auth.refreshProfile())
  ipc.handle(HOST_CHANNELS.auth.openWebsite, (_event, page) => {
    if (page !== 'register' && page !== 'reset' && page !== 'profile')
      throw new Error('Invalid account page')
    return shell.openExternal(ACCOUNT_PAGES[page])
  })
  const unsubscribe = auth.subscribe((state) => {
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed() && !window.webContents.isDestroyed()) {
        window.webContents.send(HOST_CHANNELS.auth.stateChanged, state)
      }
    }
  })
  const timer = setInterval(() => {
    void auth.refreshProfile(false)
  }, 5 * 60_000)
  timer.unref()
  return () => {
    clearInterval(timer)
    unsubscribe()
    for (const channel of Object.values(HOST_CHANNELS.auth)) ipcMain.removeHandler(channel)
  }
}
