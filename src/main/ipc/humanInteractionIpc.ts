import { BrowserWindow } from 'electron'
import { captureHostInvocation, HOST_CHANNELS } from '@mycopilot/host-api'
import {
  parseHumanInteractionSettingsGetInput,
  parseHumanInteractionSettings,
  parseHumanInteractionSettingsUpdate,
  parseHumanInteractionListInput,
  parseHumanInteractionListOutput,
  parseHumanInteractionSubmitInput,
  parseHumanInteractionIgnoreInput,
  parseHumanInteractionRequestSnapshot
} from '@mycopilot/protocol'
import type { CoreServer } from '../core/coreServer'
import type { TrustedIpcMain } from './trustedIpc'

function broadcast(channel: string, value: unknown): void {
  for (const window of BrowserWindow.getAllWindows()) {
    if (window.isDestroyed() || window.webContents.isDestroyed()) continue
    try {
      window.webContents.send(channel, value)
    } catch {
      console.warn('Failed to broadcast a human interaction update')
    }
  }
}

export function registerHumanInteractionIpc(
  ipcMain: TrustedIpcMain,
  coreServer: CoreServer
): () => void {
  ipcMain.handle(HOST_CHANNELS.humanInteraction.getSettings, (_event, input) =>
    captureHostInvocation(async () =>
      parseHumanInteractionSettings(
        await coreServer.getHumanInteractionSettings(parseHumanInteractionSettingsGetInput(input))
      )
    )
  )
  ipcMain.handle(HOST_CHANNELS.humanInteraction.updateSettings, (_event, input) =>
    captureHostInvocation(async () =>
      parseHumanInteractionSettings(
        await coreServer.updateHumanInteractionSettings(parseHumanInteractionSettingsUpdate(input))
      )
    )
  )
  ipcMain.handle(HOST_CHANNELS.humanInteraction.listRequests, (_event, input) =>
    captureHostInvocation(async () =>
      parseHumanInteractionListOutput(
        await coreServer.listHumanInteractionRequests(parseHumanInteractionListInput(input))
      )
    )
  )
  ipcMain.handle(HOST_CHANNELS.humanInteraction.submit, (_event, input) =>
    captureHostInvocation(async () =>
      parseHumanInteractionRequestSnapshot(
        await coreServer.submitHumanInteractionRequest(parseHumanInteractionSubmitInput(input))
      )
    )
  )
  ipcMain.handle(HOST_CHANNELS.humanInteraction.ignore, (_event, input) =>
    captureHostInvocation(async () =>
      parseHumanInteractionRequestSnapshot(
        await coreServer.ignoreHumanInteractionRequest(parseHumanInteractionIgnoreInput(input))
      )
    )
  )
  const stopSettings = coreServer.onHumanInteractionSettingsChanged((value) => {
    try {
      broadcast(
        HOST_CHANNELS.humanInteraction.settingsChanged,
        parseHumanInteractionSettings(value)
      )
    } catch {
      console.warn('Ignored invalid human interaction settings update')
    }
  })
  const stopRequests = coreServer.onHumanInteractionRequestChanged((value) => {
    try {
      broadcast(
        HOST_CHANNELS.humanInteraction.requestChanged,
        parseHumanInteractionRequestSnapshot(value)
      )
    } catch {
      console.warn('Ignored invalid human interaction request update')
    }
  })
  // Collaboration resync is emitted for each Core connection, independently of any open tree.
  // Translate only the lifecycle hint; question facts always come from their own query API.
  const stopResync = coreServer.onCollaborationResync?.(() => {
    broadcast(HOST_CHANNELS.humanInteraction.resync, null)
  })
  let disposed = false
  return () => {
    if (disposed) return
    disposed = true
    stopSettings()
    stopRequests()
    stopResync?.()
  }
}
