import type { StorageProjectRecord } from '@mycopilot/protocol'
import { HOST_CHANNELS } from '@mycopilot/host-api'
import type { TerminalBridge } from '../terminal/TerminalBridge'
import type { TrustedIpcMain } from './trustedIpc'

export function registerTerminalIpc(
  ipcMain: TrustedIpcMain,
  terminalBridge: TerminalBridge,
  options: { loadProjects: () => Promise<StorageProjectRecord[]> }
): void {
  terminalBridge.setProjectLoader(options.loadProjects)
  ipcMain.handle(HOST_CHANNELS.terminal.selectSourceDirectory, (event, sessionId, folderId) =>
    terminalBridge.selectSourceDirectory(event.sender, String(sessionId), String(folderId))
  )
  ipcMain.on(HOST_CHANNELS.terminal.markUserInput, (event, sessionId) =>
    terminalBridge.markUserInput(event.sender, String(sessionId))
  )
  ipcMain.handle(HOST_CHANNELS.terminal.createSession, (event, request) =>
    terminalBridge.createSession(event.sender, request)
  )
  ipcMain.on(HOST_CHANNELS.terminal.writeInput, (event, sessionId, data, userInitiated) => {
    terminalBridge.writeInput(
      event.sender,
      String(sessionId),
      typeof data === 'string' ? data : '',
      userInitiated !== false
    )
  })
  ipcMain.on(HOST_CHANNELS.terminal.acknowledgeOutput, (event, sessionId, sequence) => {
    terminalBridge.acknowledgeOutput(event.sender, String(sessionId), Number(sequence))
  })
  ipcMain.handle(HOST_CHANNELS.terminal.resizeSession, (event, sessionId, cols, rows) =>
    terminalBridge.resizeSession(event.sender, sessionId, cols, rows)
  )
  ipcMain.handle(HOST_CHANNELS.terminal.killSession, (event, sessionId) =>
    terminalBridge.killSession(event.sender, sessionId)
  )
}
