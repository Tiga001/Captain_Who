import type {
  TerminalCreateSessionRequest,
  TerminalCreateSessionResult,
  TerminalExitEvent,
  TerminalOutputEvent,
  TerminalSessionSnapshot,
  TerminalSourceFolder
} from '@mycopilot/protocol'

export type {
  TerminalCreateSessionRequest,
  TerminalCreateSessionResult,
  TerminalExitEvent,
  TerminalOutputEvent,
  TerminalSessionSnapshot,
  TerminalSourceFolder
}

export type TerminalSessionStatus = 'starting' | 'running' | 'exited' | 'error'
