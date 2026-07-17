export interface TerminalCreateSessionRequest {
  cols?: number
  cwd?: string
  rows?: number
  sessionId?: string
}

export interface TerminalSessionSnapshot {
  cols: number
  cwd: string
  processId?: number | null
  rows: number
  sessionId: string
  shell: string
}

export interface TerminalOutputEvent {
  data: string
  /** Monotonically increasing within a terminal session, starting at 1. */
  sequence: number
  sessionId: string
}

export interface TerminalExitEvent {
  exitCode?: number | null
  /** Last output sequence emitted before this terminal exit event. */
  finalOutputSequence: number
  sessionId: string
  signal?: string | null
}
