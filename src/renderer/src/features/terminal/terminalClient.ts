// Renderer terminal UI.
import { hostClient } from '../../host/hostClient'
import type {
  TerminalCreateSessionRequest,
  TerminalExitEvent,
  TerminalOutputEvent,
  TerminalSessionSnapshot
} from './terminalTypes'

export function createTerminalSession(
  request: TerminalCreateSessionRequest
): Promise<TerminalSessionSnapshot> {
  return hostClient.terminal.createSession(request)
}

export function writeTerminalInput(sessionId: string, data: string): Promise<void> {
  return hostClient.terminal.writeInput(sessionId, data)
}

export function resizeTerminalSession(
  sessionId: string,
  cols: number,
  rows: number
): Promise<void> {
  return hostClient.terminal.resizeSession(sessionId, cols, rows)
}

export function killTerminalSession(sessionId: string): Promise<boolean> {
  return hostClient.terminal.killSession(sessionId)
}

export function listenToTerminalOutput(
  handler: (event: TerminalOutputEvent) => void
): Promise<() => void> {
  return Promise.resolve(hostClient.terminal.onOutput(handler))
}

export function listenToTerminalExit(
  handler: (event: TerminalExitEvent) => void
): Promise<() => void> {
  return Promise.resolve(hostClient.terminal.onExit(handler))
}
