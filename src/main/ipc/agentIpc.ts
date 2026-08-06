import { BrowserWindow } from 'electron'
import { captureHostInvocation, HOST_CHANNELS } from '@mycopilot/host-api'
import type { CoreServer } from '../core/coreServer'
import type { TrustedIpcMain } from './trustedIpc'

export function registerAgentIpc(ipcMain: TrustedIpcMain, coreServer: CoreServer): void {
  coreServer.onAgentEvent((event) => {
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed() && !window.webContents.isDestroyed()) {
        window.webContents.send(HOST_CHANNELS.agent.event, event)
      }
    }
  })

  ipcMain.handle(HOST_CHANNELS.agent.startConversationTurn, (_event, input) =>
    captureHostInvocation(() => coreServer.startConversationTurn(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.getContextWindowSnapshot, (_event, input) =>
    captureHostInvocation(() => coreServer.getContextWindowSnapshot(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.listCommandSessions, (_event, input) =>
    captureHostInvocation(() => coreServer.listCommandSessions(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.getCommandSession, (_event, input) =>
    captureHostInvocation(() => coreServer.getCommandSession(input))
  )
  ipcMain.handle(HOST_CHANNELS.agent.steerRun, (_event, input) => coreServer.steerRun(input))
  ipcMain.handle(HOST_CHANNELS.agent.cancelRun, (_event, input) => coreServer.cancelRun(input))
  ipcMain.handle(HOST_CHANNELS.agent.listPendingActions, () => coreServer.listPendingActions())
  ipcMain.handle(HOST_CHANNELS.agent.approveAction, (_event, input) =>
    coreServer.approveAction(input)
  )
  ipcMain.handle(HOST_CHANNELS.agent.rejectAction, (_event, input) =>
    coreServer.rejectAction(input)
  )
  ipcMain.handle(HOST_CHANNELS.agent.cancelAction, (_event, input) =>
    coreServer.cancelAction(input)
  )
  ipcMain.handle(HOST_CHANNELS.agent.getUsageSummary, (_event, input) =>
    coreServer.getUsageSummary(input)
  )
  ipcMain.handle(HOST_CHANNELS.agent.clearUsageRecords, (_event, input) =>
    coreServer.clearUsageRecords(input)
  )
  ipcMain.handle(HOST_CHANNELS.agent.readFileDraft, (_event, input) =>
    coreServer.readFileDraft(input)
  )
  ipcMain.handle(HOST_CHANNELS.agent.getFileWriteDiff, (_event, input) =>
    coreServer.getFileWriteDiff(input)
  )
}
