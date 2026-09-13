import type { IpcRenderer } from 'electron'
import { HOST_CHANNELS, type AuthHostApi, type AuthState } from '@mycopilot/host-api'

export function createAuthIpcBridge(ipc: IpcRenderer): AuthHostApi {
  return {
    getState: () => ipc.invoke(HOST_CHANNELS.auth.getState),
    restoreSession: () => ipc.invoke(HOST_CHANNELS.auth.restoreSession),
    login: (input) => ipc.invoke(HOST_CHANNELS.auth.login, input),
    sendEmailCode: (email) => ipc.invoke(HOST_CHANNELS.auth.sendEmailCode, email),
    verifyEmailCode: (input) => ipc.invoke(HOST_CHANNELS.auth.verifyEmailCode, input),
    logout: () => ipc.invoke(HOST_CHANNELS.auth.logout),
    refreshProfile: () => ipc.invoke(HOST_CHANNELS.auth.refreshProfile),
    openWebsite: (page) => ipc.invoke(HOST_CHANNELS.auth.openWebsite, page),
    onStateChanged: (handler) => {
      const listener = (_event: Electron.IpcRendererEvent, state: AuthState): void => handler(state)
      ipc.on(HOST_CHANNELS.auth.stateChanged, listener)
      return () => {
        ipc.removeListener(HOST_CHANNELS.auth.stateChanged, listener)
      }
    }
  }
}
