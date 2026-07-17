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

export function acknowledgeTerminalOutput(sessionId: string, sequence: number): void {
  hostClient.terminal.acknowledgeOutput(sessionId, sequence)
}

export function writeTerminalInput(sessionId: string, data: string): void {
  hostClient.terminal.writeInput(sessionId, data)
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

export function subscribeTerminalSession(
  sessionId: string,
  handlers: {
    onExit: (event: TerminalExitEvent) => void
    onOutput: (event: TerminalOutputEvent) => void
  }
): () => void {
  return hostClient.terminal.subscribeSession(sessionId, handlers)
}
