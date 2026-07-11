import type {
  TerminalCreateSessionRequest,
  TerminalExitEvent,
  TerminalOutputEvent,
  TerminalSessionSnapshot
} from '@mycopilot/protocol'

export type {
  TerminalCreateSessionRequest,
  TerminalExitEvent,
  TerminalOutputEvent,
  TerminalSessionSnapshot
}

export type TerminalSessionStatus = 'starting' | 'running' | 'exited' | 'error'
