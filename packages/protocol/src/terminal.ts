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
  sessionId: string
}

export interface TerminalExitEvent {
  exitCode?: number | null
  sessionId: string
  signal?: string | null
}
