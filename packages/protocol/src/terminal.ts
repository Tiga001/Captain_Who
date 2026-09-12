export interface TerminalCreateSessionRequest {
  cols?: number
  cwd?: string
  /** Main resolves this project's primary folder; supplied cwd is ignored. */
  projectId?: string
  rows?: number
  sessionId?: string
}

/** Display fields from the same Host snapshot that authorizes directory selection. */
export interface TerminalSourceFolder {
  id: string
  alias: string
  path: string
  role: 'primary' | 'auxiliary'
}

export interface TerminalSessionSnapshot {
  cols: number
  cwd: string
  processId?: number | null
  rows: number
  sessionId: string
  shell: string
  sourceFolders?: TerminalSourceFolder[]
}

/** Cancellation is a normal lifecycle outcome, not an IPC handler failure. */
export type TerminalCreateSessionResult =
  { status: 'created'; session: TerminalSessionSnapshot } | { status: 'cancelled' }

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
