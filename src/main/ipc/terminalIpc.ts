import { HOST_CHANNELS } from '@mycopilot/host-api'
import type { TerminalBridge } from '../terminal/TerminalBridge'
import type { TrustedIpcMain } from './trustedIpc'

export function registerTerminalIpc(ipcMain: TrustedIpcMain, terminalBridge: TerminalBridge): void {
  ipcMain.handle(HOST_CHANNELS.terminal.createSession, (event, request) =>
    terminalBridge.createSession(event.sender, request)
  )
  ipcMain.on(HOST_CHANNELS.terminal.writeInput, (event, sessionId, data) => {
    terminalBridge.writeInput(event.sender, String(sessionId), typeof data === 'string' ? data : '')
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
